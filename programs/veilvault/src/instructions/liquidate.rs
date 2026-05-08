use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::{
    constants::{BPS_SCALER, MAX_LIQUIDATION_CLOSE_FACTOR_PCT, RATE_SCALE},
    error::LendingError,
    state::{LendingMarket, Obligation, Reserve},
};

#[derive(Accounts)]
pub struct Liquidate<'info> {
    #[account(mut)]
    pub liquidator: Signer<'info>,

    #[account(
        seeds = [b"lending_market", lending_market.owner.as_ref()],
        bump = lending_market.bump,
        constraint = !lending_market.is_paused() @ LendingError::InvalidConfig,
    )]
    pub lending_market: Box<Account<'info, LendingMarket>>,

    /// The unhealthy obligation being liquidated.
    #[account(
        mut,
        has_one = lending_market,
    )]
    pub obligation: AccountLoader<'info, Obligation>,

    // ── Repay side (debt reserve) ─────────────────────────────────────────────
    /// Reserve whose debt the liquidator is repaying.
    #[account(
        mut,
        constraint = repay_reserve.load()?.lending_market == lending_market.key() @ LendingError::InvalidConfig,
    )]
    pub repay_reserve: AccountLoader<'info, Reserve>,

    pub repay_reserve_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        seeds = [b"liquidity_vault", repay_reserve.key().as_ref()],
        bump,
    )]
    pub repay_liquidity_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Liquidator's token account that supplies the repayment.
    #[account(
        mut,
        token::mint = repay_reserve_mint,
    )]
    pub liquidator_repay_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    // ── Withdraw side (collateral reserve) ───────────────────────────────────
    /// Reserve whose cToken collateral is being seized.
    #[account(
        mut,
        constraint = withdraw_reserve.load()?.lending_market == lending_market.key() @ LendingError::InvalidConfig,
    )]
    pub withdraw_reserve: AccountLoader<'info, Reserve>,

    /// cToken mint for the collateral reserve — lending_market is mint authority.
    #[account(
        mut,
        seeds = [b"collateral_mint", withdraw_reserve.key().as_ref()],
        bump,
    )]
    pub withdraw_collateral_mint: Box<InterfaceAccount<'info, Mint>>,

    /// PDA vault holding the borrower's locked cTokens — lending_market is authority.
    #[account(
        mut,
        seeds = [b"collateral_supply", withdraw_reserve.key().as_ref()],
        bump,
    )]
    pub collateral_supply_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Liquidator's cToken account that receives the seized collateral.
    #[account(
        mut,
        token::mint = withdraw_collateral_mint,
    )]
    pub liquidator_collateral_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}    

pub fn liquidate(ctx: Context<Liquidate>, repay_amount: u64) -> Result<()> {
    require!(repay_amount > 0, LendingError::InvalidAmount);

    let clock = Clock::get()?;
    let lending_market_owner = ctx.accounts.lending_market.owner;
    let lending_market_bump = ctx.accounts.lending_market.bump;
    let repay_mint_decimals = ctx.accounts.repay_reserve_mint.decimals;
    let collateral_mint_decimals = ctx.accounts.withdraw_collateral_mint.decimals;
    let repay_reserve_key = ctx.accounts.repay_reserve.key();
    let withdraw_reserve_key = ctx.accounts.withdraw_reserve.key();

    // ── Repay reserve: accrue interest, snapshot price + cumulative rate ───────
    let (repay_price_sf, repay_cumulative_rate_sf) = {
        let mut rr = ctx.accounts.repay_reserve.load_mut()?;
        require!(rr.config.is_active(), LendingError::InvalidConfig);
        rr.accrue_interest(clock.slot)?;
        let price = rr.liquidity.market_price_sf;
        require!(price > 0, LendingError::PriceNotValid);
        (price, rr.liquidity.cumulative_borrow_rate_sf)
    };

    // ── Withdraw reserve: accrue interest, snapshot price + exchange rate ──────
    let (withdraw_price_sf, withdraw_total_liquidity, withdraw_ctoken_supply, liq_bonus_bps) = {
        let mut wr = ctx.accounts.withdraw_reserve.load_mut()?;
        require!(wr.config.is_active(), LendingError::InvalidConfig);
        wr.accrue_interest(clock.slot)?;
        let price = wr.liquidity.market_price_sf;
        require!(price > 0, LendingError::PriceNotValid);
        (
            price,
            wr.liquidity.total_supply()?,
            wr.collateral.mint_total_supply as u128,
            wr.config.liquidation_bonus_pct as u128,
        )
    };

    // ── Validate obligation and compute liquidation amounts ────────────────────
    let (actual_repay, ctokens_to_seize) = {
        let mut obligation = ctx.accounts.obligation.load_mut()?;

        require!(
            !obligation.last_update.is_slot_stale(clock.slot),
            LendingError::ObligationStale
        );
        require!(!obligation.is_healthy(), LendingError::ObligationHealthy);

        // bring borrow slot debt current with the reserve's accumulated rate
        let borrow_idx = obligation.find_borrow(repay_reserve_key)?;
        obligation.accrue_interest(borrow_idx, repay_cumulative_rate_sf)?;

        let raw_debt = obligation.borrows[borrow_idx].borrowed_amount_sf / RATE_SCALE;

        // close factor: cap each liquidation at 50% of the outstanding debt
        let max_repay = (raw_debt * MAX_LIQUIDATION_CLOSE_FACTOR_PCT / 100) as u64;
        let actual_repay = repay_amount.min(max_repay);
        require!(actual_repay > 0, LendingError::InvalidAmount);

        // repay_value_sf = repay_tokens × price_per_token_sf  [USD × RATE_SCALE]
        let repay_value_sf = (actual_repay as u128)
            .checked_mul(repay_price_sf)
            .ok_or(LendingError::MathOverflow)?;

        // apply liquidation bonus: value_with_bonus = repay_value × (10000 + bonus_bps) / 10000
        let bonus_value_sf = repay_value_sf
            .checked_mul(BPS_SCALER as u128 + liq_bonus_bps)
            .and_then(|v| v.checked_div(BPS_SCALER as u128))
            .ok_or(LendingError::MathOverflow)?;

        // collateral underlying tokens = bonus_usd_value / collateral_price_per_token
        // (RATE_SCALE cancels: [USD × RATE_SCALE] / [USD/token × RATE_SCALE] = tokens)
        let underlying_collateral = bonus_value_sf
            .checked_div(withdraw_price_sf)
            .ok_or(LendingError::MathOverflow)?;

        // convert underlying → cTokens via exchange rate (ctoken_supply / total_liquidity)
        let ctokens = if withdraw_ctoken_supply == 0 || withdraw_total_liquidity == 0 {
            underlying_collateral
        } else {
            underlying_collateral
                .checked_mul(withdraw_ctoken_supply)
                .and_then(|v| v.checked_div(withdraw_total_liquidity))
                .ok_or(LendingError::MathOverflow)?
        };

        // cap at what the obligation actually has deposited for this reserve
        let deposit_idx = obligation.find_deposit(withdraw_reserve_key)?;
        let deposited = obligation.deposits[deposit_idx].deposited_amount as u128;
        let ctokens_to_seize = ctokens.min(deposited) as u64;
        require!(ctokens_to_seize > 0, LendingError::InvalidAmount);

        obligation.repay(repay_reserve_key, actual_repay as u128)?;
        obligation.withdraw(withdraw_reserve_key, ctokens_to_seize)?;

        (actual_repay, ctokens_to_seize)
    };

    // ── Update repay reserve: reduce outstanding debt ─────────────────────────
    {
        let mut rr = ctx.accounts.repay_reserve.load_mut()?;
        rr.repay(actual_repay)?;
    }

    let signer_seeds: &[&[&[u8]]] = &[&[
        b"lending_market",
        lending_market_owner.as_ref(),
        &[lending_market_bump],
    ]];

    // CPI 1: liquidator → repay_liquidity_vault  (liquidator signs)
    token_interface::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx
                    .accounts
                    .liquidator_repay_token_account
                    .to_account_info(),
                mint: ctx.accounts.repay_reserve_mint.to_account_info(),
                to: ctx.accounts.repay_liquidity_vault.to_account_info(),
                authority: ctx.accounts.liquidator.to_account_info(),
            },
        ),
        actual_repay,
        repay_mint_decimals,
    )?;

    // CPI 2: collateral_supply_vault → liquidator_collateral_account  (lending_market PDA signs)
    token_interface::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.collateral_supply_vault.to_account_info(),
                mint: ctx.accounts.withdraw_collateral_mint.to_account_info(),
                to: ctx.accounts.liquidator_collateral_account.to_account_info(),
                authority: ctx.accounts.lending_market.to_account_info(),
            },
            signer_seeds,
        ),
        ctokens_to_seize,
        collateral_mint_decimals,
    )?;

    Ok(())
}
