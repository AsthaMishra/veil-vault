use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::{
    error::LendingError,
    state::{LendingMarket, Obligation, Reserve},
};

#[derive(Accounts)]
pub struct Repay<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,

    #[account(
        seeds = [b"lending_market", lending_market.owner.as_ref()],
        bump = lending_market.bump,
    )]
    pub lending_market: Box<Account<'info, LendingMarket>>,

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

    #[account(
        mut,
        seeds = [b"liquidity_vault", reserve.key().as_ref()],
        bump,
    )]
    pub liquidity_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    // borrower pays from their own token account
    #[account(
        mut,
        token::mint = reserve_mint,
        token::authority = borrower,
    )]
    pub user_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn repay(ctx: Context<Repay>, amount: u64) -> Result<()> {
    require!(amount > 0, LendingError::InvalidAmount);

    let clock = Clock::get()?;
    let reserve_mint_decimals = ctx.accounts.reserve_mint.decimals;
    let reserve_key = ctx.accounts.reserve.key();

    // accrue interest so the rate used for obligation accrual is up to date,
    // then update reserve: debt decreases, available_amount increases
    let cumulative_borrow_rate_sf = {
        let mut reserve = ctx.accounts.reserve.load_mut()?;
        require!(reserve.config.is_active(), LendingError::InvalidConfig);
        reserve.accrue_interest(clock.slot)?;
        let rate = reserve.liquidity.cumulative_borrow_rate_sf;
        reserve.repay(amount)?;
        rate
    };

    // accrue obligation interest with the current rate, then reduce debt
    {
        let mut obligation = ctx.accounts.obligation.load_mut()?;
        let slot_idx = obligation.find_borrow(reserve_key)?;
        obligation.accrue_interest(slot_idx, cumulative_borrow_rate_sf)?;
        obligation.repay(reserve_key, amount as u128)?;
    }

    // CPI: transfer underlying tokens from borrower → liquidity_vault (borrower signs)
    token_interface::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.user_token_account.to_account_info(),
                mint: ctx.accounts.reserve_mint.to_account_info(),
                to: ctx.accounts.liquidity_vault.to_account_info(),
                authority: ctx.accounts.borrower.to_account_info(),
            },
        ),
        amount,
        reserve_mint_decimals,
    )?;

    Ok(())
}
