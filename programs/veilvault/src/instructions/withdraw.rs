use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Burn, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::{
    error::LendingError,
    state::{LendingMarket, Reserve},
};

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub depositor: Signer<'info>,

    #[account(
        seeds = [b"lending_market", lending_market.owner.as_ref()],
        bump = lending_market.bump,
        constraint = !lending_market.is_paused() @ LendingError::InvalidConfig,
    )]
    pub lending_market: Box<Account<'info, LendingMarket>>,

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

    #[account(
        mut,
        seeds = [b"collateral_mint", reserve.key().as_ref()],
        bump,
    )]
    pub collateral_mint: Box<InterfaceAccount<'info, Mint>>,

    // depositor's cToken account — they burn from here
    #[account(
        mut,
        token::mint = collateral_mint,
        token::authority = depositor,
    )]
    pub user_collateral_account: Box<InterfaceAccount<'info, TokenAccount>>,

    // depositor's underlying token account — receives redeemed tokens
    #[account(
        mut,
        token::mint = reserve_mint,
        token::authority = depositor,
    )]
    pub user_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn withdraw(ctx: Context<Withdraw>, collateral_amount: u64) -> Result<()> {
    require!(collateral_amount > 0, LendingError::InvalidAmount);

    let clock = Clock::get()?;
    let lending_market_owner = ctx.accounts.lending_market.owner;
    let lending_market_bump = ctx.accounts.lending_market.bump;
    let reserve_mint_decimals = ctx.accounts.reserve_mint.decimals;

    // accrue interest so exchange rate is current, then compute liquidity to return
    let liquidity_amount = {
        let mut reserve = ctx.accounts.reserve.load_mut()?;
        require!(reserve.config.is_active(), LendingError::InvalidConfig);
        reserve.accrue_interest(clock.slot)?;
        reserve.redeem_collateral(collateral_amount)?
    };

    let signer_seeds: &[&[&[u8]]] = &[&[
        b"lending_market",
        lending_market_owner.as_ref(),
        &[lending_market_bump],
    ]];

    // burn cTokens from depositor's collateral account (depositor signs as token account owner)
    token_interface::burn(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Burn {
                mint: ctx.accounts.collateral_mint.to_account_info(),
                from: ctx.accounts.user_collateral_account.to_account_info(),
                authority: ctx.accounts.depositor.to_account_info(),
            },
        ),
        collateral_amount,
    )?;

    // transfer underlying tokens: liquidity_vault → depositor (lending_market signs)
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
        liquidity_amount,
        reserve_mint_decimals,
    )?;

    Ok(())
}
