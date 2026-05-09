use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};
use arcium_anchor::prelude::*;
use arcium_client::idl::arcium::types::CallbackAccount;

use crate::{
    constants::{BPS_SCALER, MAX_LIQUIDATION_CLOSE_FACTOR_PCT, RATE_SCALE},
    error::LendingError,
    state::{LendingMarket, LiquidatableEvent, Obligation, PrivateObligation, Reserve},
    ArciumSignerAccount, COMP_DEF_OFFSET_CHECK_HEALTH, ID, ID_CONST,
};
use crate::validate_callback_ixs;
use crate::LendingError as ErrorCode;

// ─── Comp-def registration ────────────────────────────────────────────────────

/// One-time admin call: registers the `check_health` Arcis circuit on-chain.
pub fn check_health_comp_def(ctx: Context<CheckHealthCompDef>) -> Result<()> {
    init_comp_def(ctx.accounts, None, None)?;
    Ok(())
}

#[init_computation_definition_accounts("check_health", payer)]
#[derive(Accounts)]
pub struct CheckHealthCompDef<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(mut, address = derive_mxe_pda!())]
    pub mxe_account: Box<Account<'info, MXEAccount>>,
    /// CHECK: initialised by arcium program.
    #[account(mut)]
    pub comp_def_account: UncheckedAccount<'info>,
    /// CHECK: address_lookup_table, checked by arcium program.
    #[account(mut, address = derive_mxe_lut_pda!(mxe_account.lut_offset_slot))]
    pub address_lookup_table: UncheckedAccount<'info>,
    #[account(address = LUT_PROGRAM_ID)]
    /// CHECK: lut_program is the Address Lookup Table program.
    pub lut_program: UncheckedAccount<'info>,
    pub arcium_program: Program<'info, Arcium>,
    pub system_program: Program<'info, System>,
}

// ─── Private check liquidatable ───────────────────────────────────────────────

/// Queues a confidential health-factor computation for a user's private position.
/// The MXE receives the encrypted position + public prices, computes HF, and calls
/// back with a bool. If HF < 1, `is_liquidatable` is set and a LiquidatableEvent emitted.
///
/// Public inputs (derived from on-chain reserve state, never the encrypted amounts):
///   `exchange_rate_bps`       — cToken→underlying rate × 10_000 (e.g., 1.02 → 10_200)
///   `collateral_price_cents`  — USD price of 1 whole collateral underlying token in cents
///   `borrow_price_cents`      — USD price of 1 whole borrow underlying token in cents
///   `ltv_bps`                 — liquidation threshold × 10_000 (e.g., 85% → 8_500)
pub fn private_check_liquidatable(
    ctx: Context<PrivateCheckLiquidatable>,
    computation_offset: u64,
    exchange_rate_bps: u64,
    collateral_price_cents: u64,
    borrow_price_cents: u64,
    ltv_bps: u64,
) -> Result<()> {
    require!(
        ctx.accounts.private_obligation.is_initialized,
        LendingError::PrivateObligationNotInitialized
    );

    ctx.accounts.sign_pda_account.bump = ctx.bumps.sign_pda_account;

    // ArgBuilder order matches circuit signature:
    //   check_health(state: Enc<Mxe, PrivatePosition>, exchange_rate_bps, ...)
    let args = ArgBuilder::new()
        .plaintext_u128(ctx.accounts.private_obligation.nonce)
        .account(
            ctx.accounts.private_obligation.key(),
            8 + 1,  // discriminator (8) + bump (1) = offset of enc_state field
            64,     // [[u8; 32]; 2] = 64 bytes
        )
        .plaintext_u64(exchange_rate_bps)
        .plaintext_u64(collateral_price_cents)
        .plaintext_u64(borrow_price_cents)
        .plaintext_u64(ltv_bps)
        .build();

    queue_computation(
        ctx.accounts,
        computation_offset,
        args,
        vec![CheckHealthCallback::callback_ix(
            computation_offset,
            &ctx.accounts.mxe_account,
            &[CallbackAccount {
                pubkey: ctx.accounts.private_obligation.key(),
                is_writable: true,
            }],
        )?],
        1,
        0,
    )?;

    Ok(())
}

#[queue_computation_accounts("check_health", liquidator)]
#[derive(Accounts)]
#[instruction(computation_offset: u64)]
pub struct PrivateCheckLiquidatable<'info> {
    #[account(mut)]
    pub liquidator: Signer<'info>,

    #[account(
        init_if_needed,
        space = 9,
        payer = liquidator,
        seeds = [&SIGN_PDA_SEED],
        bump,
        address = derive_sign_pda!(),
    )]
    pub sign_pda_account: Account<'info, ArciumSignerAccount>,

    #[account(address = derive_mxe_pda!())]
    pub mxe_account: Box<Account<'info, MXEAccount>>,

    #[account(
        mut,
        address = derive_mempool_pda!(mxe_account, LendingError::ClusterNotSet)
    )]
    /// CHECK: checked by arcium program.
    pub mempool_account: UncheckedAccount<'info>,

    #[account(
        mut,
        address = derive_execpool_pda!(mxe_account, LendingError::ClusterNotSet)
    )]
    /// CHECK: checked by arcium program.
    pub executing_pool: UncheckedAccount<'info>,

    #[account(
        mut,
        address = derive_comp_pda!(computation_offset, mxe_account, LendingError::ClusterNotSet)
    )]
    /// CHECK: checked by arcium program.
    pub computation_account: UncheckedAccount<'info>,

    #[account(address = derive_comp_def_pda!(COMP_DEF_OFFSET_CHECK_HEALTH))]
    pub comp_def_account: Box<Account<'info, ComputationDefinitionAccount>>,

    #[account(
        mut,
        address = derive_cluster_pda!(mxe_account, LendingError::ClusterNotSet)
    )]
    pub cluster_account: Box<Account<'info, Cluster>>,

    #[account(mut, address = ARCIUM_FEE_POOL_ACCOUNT_ADDRESS)]
    pub pool_account: Box<Account<'info, FeePool>>,

    #[account(mut, address = ARCIUM_CLOCK_ACCOUNT_ADDRESS)]
    pub clock_account: Box<Account<'info, ClockAccount>>,

    pub system_program: Program<'info, System>,
    pub arcium_program: Program<'info, Arcium>,

    #[account(
        seeds = [b"lending_market", lending_market.owner.as_ref()],
        bump = lending_market.bump,
    )]
    pub lending_market: Box<Account<'info, LendingMarket>>,

    /// CHECK: PDA seed on private_obligation verifies this is the correct owner.
    pub obligation_owner: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [
            b"private_obligation",
            lending_market.key().as_ref(),
            obligation_owner.key().as_ref(),
        ],
        bump = private_obligation.bump,
    )]
    pub private_obligation: Box<Account<'info, PrivateObligation>>,
}

// ─── Callback ─────────────────────────────────────────────────────────────────

/// Receives the bool result from the MXE.
/// Sets `is_liquidatable = true` and emits LiquidatableEvent if HF < 1,
/// or clears the flag if the position has recovered.
#[arcium_callback(encrypted_ix = "check_health")]
pub fn check_health_callback(
    ctx: Context<CheckHealthCallback>,
    output: SignedComputationOutputs<CheckHealthOutput>,
) -> Result<()> {
    let is_healthy = match output.verify_output(
        &ctx.accounts.cluster_account,
        &ctx.accounts.computation_account,
    ) {
        Ok(CheckHealthOutput { field_0 }) => field_0,
        Err(_) => return Err(LendingError::AbortedComputation.into()),
    };

    if !is_healthy {
        ctx.accounts.private_obligation.is_liquidatable = true;
        emit!(LiquidatableEvent {
            private_obligation: ctx.accounts.private_obligation.key(),
            owner: ctx.accounts.private_obligation.owner,
        });
    } else {
        ctx.accounts.private_obligation.is_liquidatable = false;
    }

    Ok(())
}

#[callback_accounts("check_health")]
#[derive(Accounts)]
pub struct CheckHealthCallback<'info> {
    pub arcium_program: Program<'info, Arcium>,

    #[account(address = derive_comp_def_pda!(COMP_DEF_OFFSET_CHECK_HEALTH))]
    pub comp_def_account: Box<Account<'info, ComputationDefinitionAccount>>,

    #[account(address = derive_mxe_pda!())]
    pub mxe_account: Box<Account<'info, MXEAccount>>,

    /// CHECK: checked by arcium program via callback context constraints.
    pub computation_account: UncheckedAccount<'info>,

    #[account(address = derive_cluster_pda!(mxe_account, LendingError::ClusterNotSet))]
    pub cluster_account: Box<Account<'info, Cluster>>,

    #[account(address = ::anchor_lang::solana_program::sysvar::instructions::ID)]
    /// CHECK: instructions sysvar.
    pub instructions_sysvar: AccountInfo<'info>,

    #[account(mut)]
    pub private_obligation: Account<'info, PrivateObligation>,
}

// ─── Execute private liquidation ─────────────────────────────────────────────

/// Seizes collateral from a position flagged as liquidatable by check_health_callback.
/// Logic mirrors the cleartext `liquidate` instruction; the only difference is that
/// the health check gate is the MPC-set `is_liquidatable` flag rather than an inline
/// obligation.is_healthy() call.
pub fn execute_private_liquidation(
    ctx: Context<ExecutePrivateLiquidation>,
    repay_amount: u64,
) -> Result<()> {
    require!(
        ctx.accounts.private_obligation.is_liquidatable,
        LendingError::ObligationHealthy
    );
    require!(repay_amount > 0, LendingError::InvalidAmount);
    require!(
        !ctx.accounts.lending_market.is_paused(),
        LendingError::InvalidConfig
    );

    let clock = Clock::get()?;
    let lending_market_owner = ctx.accounts.lending_market.owner;
    let lending_market_bump = ctx.accounts.lending_market.bump;
    let repay_mint_decimals = ctx.accounts.repay_reserve_mint.decimals;
    let collateral_mint_decimals = ctx.accounts.withdraw_collateral_mint.decimals;
    let repay_reserve_key = ctx.accounts.repay_reserve.key();
    let withdraw_reserve_key = ctx.accounts.withdraw_reserve.key();

    let (repay_price_sf, repay_cumulative_rate_sf) = {
        let mut rr = ctx.accounts.repay_reserve.load_mut()?;
        require!(rr.config.is_active(), LendingError::InvalidConfig);
        rr.accrue_interest(clock.slot)?;
        let price = rr.liquidity.market_price_sf;
        require!(price > 0, LendingError::PriceNotValid);
        (price, rr.liquidity.cumulative_borrow_rate_sf)
    };

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

    let (actual_repay, ctokens_to_seize) = {
        let mut obligation = ctx.accounts.obligation.load_mut()?;

        require!(
            !obligation.last_update.is_slot_stale(clock.slot),
            LendingError::ObligationStale
        );

        let borrow_idx = obligation.find_borrow(repay_reserve_key)?;
        obligation.accrue_interest(borrow_idx, repay_cumulative_rate_sf)?;

        let raw_debt = obligation.borrows[borrow_idx].borrowed_amount_sf / RATE_SCALE;
        let max_repay = (raw_debt * MAX_LIQUIDATION_CLOSE_FACTOR_PCT / 100) as u64;
        let actual_repay = repay_amount.min(max_repay);
        require!(actual_repay > 0, LendingError::InvalidAmount);

        let repay_value_sf = (actual_repay as u128)
            .checked_mul(repay_price_sf)
            .ok_or(LendingError::MathOverflow)?;

        let bonus_value_sf = repay_value_sf
            .checked_mul(BPS_SCALER as u128 + liq_bonus_bps)
            .and_then(|v| v.checked_div(BPS_SCALER as u128))
            .ok_or(LendingError::MathOverflow)?;

        let underlying_collateral = bonus_value_sf
            .checked_div(withdraw_price_sf)
            .ok_or(LendingError::MathOverflow)?;

        let ctokens = if withdraw_ctoken_supply == 0 || withdraw_total_liquidity == 0 {
            underlying_collateral
        } else {
            underlying_collateral
                .checked_mul(withdraw_ctoken_supply)
                .and_then(|v| v.checked_div(withdraw_total_liquidity))
                .ok_or(LendingError::MathOverflow)?
        };

        let deposit_idx = obligation.find_deposit(withdraw_reserve_key)?;
        let deposited = obligation.deposits[deposit_idx].deposited_amount as u128;
        let ctokens_to_seize = ctokens.min(deposited) as u64;
        require!(ctokens_to_seize > 0, LendingError::InvalidAmount);

        obligation.repay(repay_reserve_key, actual_repay as u128)?;
        obligation.withdraw(withdraw_reserve_key, ctokens_to_seize)?;

        (actual_repay, ctokens_to_seize)
    };

    {
        let mut rr = ctx.accounts.repay_reserve.load_mut()?;
        rr.repay(actual_repay)?;
    }

    let signer_seeds: &[&[&[u8]]] = &[&[
        b"lending_market",
        lending_market_owner.as_ref(),
        &[lending_market_bump],
    ]];

    // Liquidator repays debt: liquidator → liquidity_vault.
    token_interface::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.liquidator_repay_account.to_account_info(),
                mint: ctx.accounts.repay_reserve_mint.to_account_info(),
                to: ctx.accounts.repay_liquidity_vault.to_account_info(),
                authority: ctx.accounts.liquidator.to_account_info(),
            },
        ),
        actual_repay,
        repay_mint_decimals,
    )?;

    // Liquidator receives seized cTokens: collateral_supply_vault → liquidator (lending_market signs).
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

    // Clear flag — position needs a fresh MPC check to be re-liquidated.
    ctx.accounts.private_obligation.is_liquidatable = false;

    Ok(())
}

#[derive(Accounts)]
pub struct ExecutePrivateLiquidation<'info> {
    #[account(mut)]
    pub liquidator: Signer<'info>,

    #[account(
        seeds = [b"lending_market", lending_market.owner.as_ref()],
        bump = lending_market.bump,
        constraint = !lending_market.is_paused() @ LendingError::InvalidConfig,
    )]
    pub lending_market: Box<Account<'info, LendingMarket>>,

    /// CHECK: PDA seed on private_obligation verifies correctness.
    pub obligation_owner: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [
            b"private_obligation",
            lending_market.key().as_ref(),
            obligation_owner.key().as_ref(),
        ],
        bump = private_obligation.bump,
    )]
    pub private_obligation: Box<Account<'info, PrivateObligation>>,

    #[account(
        mut,
        has_one = lending_market,
    )]
    pub obligation: AccountLoader<'info, Obligation>,

    // ── Repay side (debt reserve) ────────────────────────────────────────────

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

    #[account(
        mut,
        token::mint = repay_reserve_mint,
        token::authority = liquidator,
    )]
    pub liquidator_repay_account: Box<InterfaceAccount<'info, TokenAccount>>,

    // ── Collateral side (withdraw reserve) ──────────────────────────────────

    #[account(
        mut,
        constraint = withdraw_reserve.load()?.lending_market == lending_market.key() @ LendingError::InvalidConfig,
    )]
    pub withdraw_reserve: AccountLoader<'info, Reserve>,

    #[account(
        mut,
        seeds = [b"collateral_mint", withdraw_reserve.key().as_ref()],
        bump,
    )]
    pub withdraw_collateral_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        seeds = [b"collateral_supply", withdraw_reserve.key().as_ref()],
        bump,
    )]
    pub collateral_supply_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = withdraw_collateral_mint,
    )]
    pub liquidator_collateral_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}
