use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::{
    error::LendingError,
    state::{
        InitReserveConfigParams, LendingMarket, NewReserveCollateralParams,
        NewReserveLiquidityParams, Reserve, RESERVE_VERSION,
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
    pub lending_market: Box<Account<'info, LendingMarket>>,

    // reserve PDA — AccountLoader avoids deserializing the large struct onto the stack
    #[account(
        init,
        payer = owner,
        space = 8 + std::mem::size_of::<Reserve>(),
        seeds = [b"reserve", lending_market.key().as_ref(), reserve_mint.key().as_ref()],
        bump,
    )]
    pub reserve: AccountLoader<'info, Reserve>,


    // underlying token mint (e.g. USDC, SOL wrapped)
    pub reserve_mint: Box<InterfaceAccount<'info, Mint>>,

    // token account that holds deposited liquidity — owned by lending_market PDA
    #[account(
        init,
        payer = owner,
        token::mint = reserve_mint,
        token::authority = lending_market,
        seeds = [b"liquidity_vault", reserve.key().as_ref()],
        bump,
    )]
    pub liquidity_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    // token account that accumulates protocol fees — owned by lending_market PDA
    #[account(
        init,
        payer = owner,
        token::mint = reserve_mint,
        token::authority = lending_market,
        seeds = [b"fee_vault", reserve.key().as_ref()],
        bump,
    )]
    pub fee_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    // cToken mint — minted to depositors, authority = lending_market PDA
    #[account(
        init,
        payer = owner,
        mint::decimals = reserve_mint.decimals,
        mint::authority = lending_market,
        seeds = [b"collateral_mint", reserve.key().as_ref()],
        bump,
    )]
    pub collateral_mint: Box<InterfaceAccount<'info, Mint>>,

    // holds users' cTokens locked as collateral — owned by lending_market PDA
    #[account(
        init,
        payer = owner,
        token::mint = collateral_mint,
        token::authority = lending_market,
        seeds = [b"collateral_supply", reserve.key().as_ref()],
        bump,
    )]
    pub collateral_supply_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

pub fn add_reserve(ctx: Context<AddReserve>, args: AddReserveArgs) -> Result<()> {
    let clock = Clock::get()?;

    // capture keys before load_init() borrows reserve
    let reserve_mint_key = ctx.accounts.reserve_mint.key();
    let liquidity_vault_key = ctx.accounts.liquidity_vault.key();
    let fee_vault_key = ctx.accounts.fee_vault.key();
    let collateral_mint_key = ctx.accounts.collateral_mint.key();
    let collateral_supply_key = ctx.accounts.collateral_supply_vault.key();
    let lending_market_key = ctx.accounts.lending_market.key();

    // zero-copy write — no stack copy of Reserve
    let mut reserve = ctx.accounts.reserve.load_init()?;

    reserve.version = RESERVE_VERSION;
    reserve.last_update_slot = clock.slot;
    reserve.bump = ctx.bumps.reserve;
    reserve.lending_market = lending_market_key;

    reserve.liquidity.init(NewReserveLiquidityParams {
        mint: reserve_mint_key,
        supply_vault: liquidity_vault_key,
        fee_vault: fee_vault_key,
    });

    reserve.collateral.init(NewReserveCollateralParams {
        mint_pda: collateral_mint_key,
        supply_vault_pda: collateral_supply_key,
    });

    reserve.config.init(args.config)?;
    // pyth_oracle is included in args.config and written by ReserveConfig::init

    Ok(())
}
