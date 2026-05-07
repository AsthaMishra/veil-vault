use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    self, Mint, MintTo, TokenAccount, TokenInterface, TransferChecked,
};

use crate::{
    error::LendingError,
    state::{LendingMarket, Reserve},
};

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(mut)]
    pub depositor: Signer<'info>,

    // not mut — we only read owner/bump and use it as a PDA signer
    #[account(
        seeds = [b"lending_market", lending_market.owner.as_ref()],
        bump = lending_market.bump,
        constraint = !lending_market.is_paused() @ LendingError::InvalidConfig,
    )]
    pub lending_market: Box<Account<'info, LendingMarket>>,

    // seeds include reserve_mint so the constraint implicitly validates the mint
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

    // source: depositor's underlying token account (e.g. USDC wallet)
    #[account(
        mut,
        token::mint = reserve_mint,
        token::authority = depositor,
    )]
    pub user_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    // destination: depositor's cToken account — must exist, create ATA beforehand
    #[account(
        mut,
        token::mint = collateral_mint,
        token::authority = depositor,
    )]
    pub user_collateral_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
    require!(amount > 0, LendingError::InvalidAmount);

    let clock = Clock::get()?;

    // capture before borrowing reserve
    let lending_market_owner = ctx.accounts.lending_market.owner;
    let lending_market_bump = ctx.accounts.lending_market.bump;
    let reserve_mint_decimals = ctx.accounts.reserve_mint.decimals;

    // accrue interest so exchange rate reflects current slot, then record deposit
    let collateral_amount = {
        let mut reserve = ctx.accounts.reserve.load_mut()?;
        require!(reserve.config.is_active(), LendingError::InvalidConfig);
        reserve.accrue_interest(clock.slot)?;
        reserve.deposit_liquidity(amount)?
    };

    // transfer underlying tokens: depositor → liquidity_vault
    token_interface::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.user_token_account.to_account_info(),
                mint: ctx.accounts.reserve_mint.to_account_info(),
                to: ctx.accounts.liquidity_vault.to_account_info(),
                authority: ctx.accounts.depositor.to_account_info(),
            },
        ),
        amount,
        reserve_mint_decimals,
    )?;

    // mint cTokens: collateral_mint → depositor's collateral account (lending_market signs)
    let signer_seeds: &[&[&[u8]]] = &[&[
        b"lending_market",
        lending_market_owner.as_ref(),
        &[lending_market_bump],
    ]];

    token_interface::mint_to(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.collateral_mint.to_account_info(),
                to: ctx.accounts.user_collateral_account.to_account_info(),
                authority: ctx.accounts.lending_market.to_account_info(),
            },
            signer_seeds,
        ),
        collateral_amount,
    )?;

    Ok(())
}
