use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};

use crate::{
    error::LendingError,
    state::{
        InitReserveConfigParams, InitReserveParams, LendingMarket, NewReserveCollateralParams,
        NewReserveLiquidityParams, Reserve, ReserveCollateral, ReserveConfig, ReserveLiquidity,
    },
};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct AddReserveArgs {
    pub config: InitReserveConfigParams,
}

#[derive(Accounts)]
pub struct AddReserve<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,

    // lending_market PDA — must be owned by signer
    #[account(
        mut,
        seeds = [b"lending_market", owner.key().as_ref()],
        bump = lending_market.bump,
        has_one = owner,
        constraint = !lending_market.is_paused() @ LendingError::InvalidConfig,
    )]
    pub lending_market: Account<'info, LendingMarket>,

    // reserve PDA — one per (market, mint) pair
    #[account(
        init,
        payer = owner,
        space = 8 + std::mem::size_of::<Reserve>(),
        seeds = [b"reserve", lending_market.key().as_ref(), reserve_mint.key().as_ref()],
        bump,
    )]
    pub reserve: Account<'info, Reserve>,

    // underlying token mint (e.g. USDC, SOL wrapped)
    pub reserve_mint: Account<'info, Mint>,

    // token account that holds deposited liquidity — owned by lending_market PDA
    #[account(
        init,
        payer = owner,
        token::mint = reserve_mint,
        token::authority = lending_market,
        seeds = [b"liquidity_vault", reserve.key().as_ref()],
        bump,
    )]
    pub liquidity_vault: Account<'info, TokenAccount>,

    // token account that accumulates protocol fees — owned by lending_market PDA
    #[account(
        init,
        payer = owner,
        token::mint = reserve_mint,
        token::authority = lending_market,
        seeds = [b"fee_vault", reserve.key().as_ref()],
        bump,
    )]
    pub fee_vault: Account<'info, TokenAccount>,

    // cToken mint — minted to depositors, authority = lending_market PDA
    #[account(
        init,
        payer = owner,
        mint::decimals = reserve_mint.decimals,
        mint::authority = lending_market,
        seeds = [b"collateral_mint", reserve.key().as_ref()],
        bump,
    )]
    pub collateral_mint: Account<'info, Mint>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn add_reserve(ctx: Context<AddReserve>, args: AddReserveArgs) -> Result<()> {
    let clock = Clock::get()?;

    // build and validate config
    let mut config = ReserveConfig::default();
    config.init(args.config)?;

    // wire liquidity vault and fee vault pubkeys into reserve state
    let mut liquidity = ReserveLiquidity::default();
    liquidity.init(NewReserveLiquidityParams {
        mint: ctx.accounts.reserve_mint.key(),
        supply_vault: ctx.accounts.liquidity_vault.key(),
        fee_vault: ctx.accounts.fee_vault.key(),
    });

    // wire collateral mint pubkey into reserve state
    let mut collateral = ReserveCollateral::default();
    collateral.init(NewReserveCollateralParams {
        mint_pda: ctx.accounts.collateral_mint.key(),
        supply_vault_pda: Pubkey::default(),
    });

    ctx.accounts.reserve.init(InitReserveParams {
        current_slot: clock.slot,
        lending_market: ctx.accounts.lending_market.key(),
        liquidity,
        collateral,
        config,
    });

    Ok(())
}
