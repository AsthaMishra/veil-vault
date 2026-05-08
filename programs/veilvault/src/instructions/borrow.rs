use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::{
    error::LendingError,
    state::{LendingMarket, Obligation, Reserve},
};

#[derive(Accounts)]
pub struct Borrow<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,

    #[account(
        seeds = [b"lending_market", lending_market.owner.as_ref()],
        bump = lending_market.bump,
        constraint = !lending_market.is_paused() @ LendingError::InvalidConfig,
    )]
    pub lending_market: Box<Account<'info, LendingMarket>>,

    // seeds enforce this obligation belongs to borrower + this market
    #[account(
        mut,
        seeds = [b"obligation", lending_market.key().as_ref(), borrower.key().as_ref()],
        bump,
    )]
    pub obligation: AccountLoader<'info, Obligation>,

    #[account(
        mut,
        seeds = [b"reserve", lending_market.key().as_ref(), reserve_mint.key().as_ref()],
        bump,
    )]
    pub reserve: AccountLoader<'info, Reserve>,

    pub reserve_mint: Box<InterfaceAccount<'info, Mint>>,

    // liquidity_vault authority = lending_market PDA, so it signs the transfer out
    #[account(
        mut,
        seeds = [b"liquidity_vault", reserve.key().as_ref()],
        bump,
    )]
    pub liquidity_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    // where the borrowed tokens land — any account with the right mint
    #[account(
        mut,
        token::mint = reserve_mint,
    )]
    pub user_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn borrow(ctx: Context<Borrow>, amount: u64) -> Result<()> {
    require!(amount > 0, LendingError::InvalidAmount);

    let clock = Clock::get()?;
    let lending_market_owner = ctx.accounts.lending_market.owner;
    let lending_market_bump = ctx.accounts.lending_market.bump;
    let reserve_mint_decimals = ctx.accounts.reserve_mint.decimals;
    let reserve_key = ctx.accounts.reserve.key();

    // accrue interest so the rate snapshot we store is current, then update reserve debt
    let (cumulative_borrow_rate_sf, reserve_price_sf) = {
        let mut reserve = ctx.accounts.reserve.load_mut()?;
        require!(reserve.config.is_active(), LendingError::InvalidConfig);
        // price must be set by refresh_reserve — prevents borrowing against stale/zero price
        require!(reserve.liquidity.market_price_sf > 0, LendingError::PriceNotValid);
        reserve.accrue_interest(clock.slot)?;
        reserve.borrow(amount)?; // checks borrow_limit + insufficient_liquidity
        (
            reserve.liquidity.cumulative_borrow_rate_sf,
            reserve.liquidity.market_price_sf,
        )
    };

    // accrue interest on any existing debt for this reserve, then record new borrow
    {
        let mut obligation = ctx.accounts.obligation.load_mut()?;

        // require a fresh refresh_obligation in the same slot
        require!(
            !obligation.last_update.is_slot_stale(clock.slot),
            LendingError::ObligationStale
        );

        if let Ok(slot_idx) = obligation.find_borrow(reserve_key) {
            obligation.accrue_interest(slot_idx, cumulative_borrow_rate_sf)?;
        }

        obligation.borrow(reserve_key, amount as u128, cumulative_borrow_rate_sf)?;

        // Inline health-factor check that accounts for the new borrow.
        //
        // obligation.health_factor() uses market_value_sf, which refresh_obligation populates
        // for existing borrows (staleness check above ensures it ran this slot). The newly
        // added borrow has market_value_sf = 0 (not yet priced), so we compute its USD value
        // inline using the reserve's current price and add it to the existing borrow total.
        let collateral_value_sf: u128 = obligation.deposits
            [..obligation.deposits_count as usize]
            .iter()
            .filter(|d| d.is_active())
            .map(|d| d.market_value_sf)
            .sum();

        let existing_borrow_value_sf: u128 = obligation.borrows
            [..obligation.borrows_count as usize]
            .iter()
            .filter(|b| b.is_active())
            .map(|b| b.market_value_sf)
            .sum();

        let new_borrow_value_sf = (amount as u128)
            .checked_mul(reserve_price_sf)
            .ok_or(LendingError::MathOverflow)?;

        let total_borrow_value_sf = existing_borrow_value_sf
            .checked_add(new_borrow_value_sf)
            .ok_or(LendingError::MathOverflow)?;

        require!(
            collateral_value_sf >= total_borrow_value_sf,
            LendingError::UnhealthyObligation
        );
    }

    // CPI: transfer underlying tokens from liquidity_vault → borrower (lending_market signs)
    let signer_seeds: &[&[&[u8]]] = &[&[
        b"lending_market",
        lending_market_owner.as_ref(),
        &[lending_market_bump],
    ]];

    token_interface::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.liquidity_vault.to_account_info(),
                mint: ctx.accounts.reserve_mint.to_account_info(),
                to: ctx.accounts.user_token_account.to_account_info(),
                authority: ctx.accounts.lending_market.to_account_info(),
            },
            signer_seeds,
        ),
        amount,
        reserve_mint_decimals,
    )?;

    Ok(())
}
