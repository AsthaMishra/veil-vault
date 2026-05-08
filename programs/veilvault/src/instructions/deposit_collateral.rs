use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::{
    error::LendingError,
    state::{LendingMarket, Obligation, Reserve},
};

#[derive(Accounts)]
pub struct DepositCollateral<'info> {
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

    // user's cToken account — tokens move out of here
    #[account(
        mut,
        token::mint = collateral_mint,
        token::authority = depositor,
    )]
    pub user_collateral_account: Box<InterfaceAccount<'info, TokenAccount>>,

    // PDA vault that holds locked collateral — lending_market is authority
    #[account(
        mut,
        seeds = [b"collateral_supply", reserve.key().as_ref()],
        bump,
    )]
    pub collateral_supply_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    // obligation seeded from (lending_market, depositor) — only owner can lock their own collateral
    #[account(
        mut,
        seeds = [b"obligation", lending_market.key().as_ref(), depositor.key().as_ref()],
        bump,
    )]
    pub obligation: AccountLoader<'info, Obligation>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn deposit_collateral(ctx: Context<DepositCollateral>, collateral_amount: u64) -> Result<()> {
    require!(collateral_amount > 0, LendingError::InvalidAmount);

    let reserve_key = ctx.accounts.reserve.key();
    let collateral_mint_decimals = ctx.accounts.collateral_mint.decimals;

    {
        let reserve = ctx.accounts.reserve.load()?;
        require!(reserve.config.is_active(), LendingError::InvalidConfig);
    }

    // record the locked collateral in the obligation before the token transfer
    {
        let mut obligation = ctx.accounts.obligation.load_mut()?;
        obligation.deposit(reserve_key, collateral_amount)?;
    }

    // transfer cTokens: user_collateral_account → collateral_supply_vault (depositor signs)
    token_interface::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.user_collateral_account.to_account_info(),
                mint: ctx.accounts.collateral_mint.to_account_info(),
                to: ctx.accounts.collateral_supply_vault.to_account_info(),
                authority: ctx.accounts.depositor.to_account_info(),
            },
        ),
        collateral_amount,
        collateral_mint_decimals,
    )?;

    Ok(())
}
