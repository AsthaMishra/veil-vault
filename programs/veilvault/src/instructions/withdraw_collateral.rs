use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::{
    constants::RATE_SCALE,
    error::LendingError,
    state::{LendingMarket, Obligation, Reserve},
};

#[derive(Accounts)]
pub struct WithdrawCollateral<'info> {
    #[account(mut)]
    pub depositor: Signer<'info>,

    #[account(
        seeds = [b"lending_market", lending_market.owner.as_ref()],
        bump = lending_market.bump,
        constraint = !lending_market.is_paused() @ LendingError::InvalidConfig,
    )]
    pub lending_market: Box<Account<'info, LendingMarket>>,

    #[account(
        seeds = [b"reserve", lending_market.key().as_ref(), reserve_mint.key().as_ref()],
        bump,
    )]
    pub reserve: AccountLoader<'info, Reserve>,

    pub reserve_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        seeds = [b"collateral_mint", reserve.key().as_ref()],
        bump,
    )]
    pub collateral_mint: Box<InterfaceAccount<'info, Mint>>,

    // PDA vault holding locked collateral — lending_market signs the transfer out
    #[account(
        mut,
        seeds = [b"collateral_supply", reserve.key().as_ref()],
        bump,
    )]
    pub collateral_supply_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    // user's cToken account — tokens return here
    #[account(
        mut,
        token::mint = collateral_mint,
        token::authority = depositor,
    )]
    pub user_collateral_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [b"obligation", lending_market.key().as_ref(), depositor.key().as_ref()],
        bump,
    )]
    pub obligation: AccountLoader<'info, Obligation>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn withdraw_collateral(
    ctx: Context<WithdrawCollateral>,
    collateral_amount: u64,
) -> Result<()> {
    require!(collateral_amount > 0, LendingError::InvalidAmount);

    let clock = Clock::get()?;
    let reserve_key = ctx.accounts.reserve.key();
    let collateral_mint_decimals = ctx.accounts.collateral_mint.decimals;
    let lending_market_owner = ctx.accounts.lending_market.owner;
    let lending_market_bump = ctx.accounts.lending_market.bump;

    {
        let mut obligation = ctx.accounts.obligation.load_mut()?;

        // prices in market_value_sf must be current — require refresh_obligation this slot
        require!(
            !obligation.last_update.is_slot_stale(clock.slot),
            LendingError::ObligationStale
        );

        let deposit_idx = obligation.find_deposit(reserve_key)?;
        let old_deposited = obligation.deposits[deposit_idx].deposited_amount as u128;
        let new_deposited = old_deposited
            .checked_sub(collateral_amount as u128)
            .ok_or(LendingError::InsufficientCollateral)?;

        // proportionally reduce market_value_sf so health_factor() reflects the withdrawal
        // before obligation.withdraw() clears the slot on full exit
        obligation.deposits[deposit_idx].market_value_sf = obligation.deposits[deposit_idx]
            .market_value_sf
            .checked_mul(new_deposited)
            .and_then(|v| v.checked_div(old_deposited.max(1)))
            .ok_or(LendingError::MathOverflow)?;

        obligation.withdraw(reserve_key, collateral_amount)?;

        // after reducing collateral, position must still be healthy (or borrow-free)
        if let Some(hf) = obligation.health_factor() {
            require!(hf >= RATE_SCALE, LendingError::UnhealthyObligation);
        }
    }

    let signer_seeds: &[&[&[u8]]] = &[&[
        b"lending_market",
        lending_market_owner.as_ref(),
        &[lending_market_bump],
    ]];

    // transfer cTokens: collateral_supply_vault → user_collateral_account (lending_market signs)
    token_interface::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.collateral_supply_vault.to_account_info(),
                mint: ctx.accounts.collateral_mint.to_account_info(),
                to: ctx.accounts.user_collateral_account.to_account_info(),
                authority: ctx.accounts.lending_market.to_account_info(),
            },
            signer_seeds,
        ),
        collateral_amount,
        collateral_mint_decimals,
    )?;

    Ok(())
}
