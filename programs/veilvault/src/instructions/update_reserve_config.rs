use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

use crate::state::{InitReserveConfigParams, LendingMarket, Reserve};

#[derive(Accounts)]
pub struct UpdateReserveConfig<'info> {
    pub owner: Signer<'info>,

    #[account(
        seeds = [b"lending_market", owner.key().as_ref()],
        bump = lending_market.bump,
        has_one = owner,
    )]
    pub lending_market: Box<Account<'info, LendingMarket>>,

    pub reserve_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        has_one = lending_market,
        seeds = [b"reserve", lending_market.key().as_ref(), reserve_mint.key().as_ref()],
        bump = reserve.load()?.bump,
    )]
    pub reserve: AccountLoader<'info, Reserve>,
}

pub fn update_reserve_config(
    ctx: Context<UpdateReserveConfig>,
    new_config: InitReserveConfigParams,
) -> Result<()> {
    let mut reserve = ctx.accounts.reserve.load_mut()?;
    reserve.config.init(new_config)
}
