use anchor_lang::prelude::*;

use crate::{
    constants::MAX_AGE_SECONDS,
    error::LendingError,
    state::{LendingMarket, Reserve},
    utils::prices::get_pyth_price,
};

#[derive(Accounts)]
pub struct RefreshReserve<'info> {
    #[account(
        seeds = [b"lending_market", lending_market.owner.as_ref()],
        bump = lending_market.bump,
    )]
    pub lending_market: Box<Account<'info, LendingMarket>>,

    #[account(
        mut,
        constraint = reserve.load()?.lending_market == lending_market.key() @ LendingError::InvalidConfig,
    )]
    pub reserve: AccountLoader<'info, Reserve>,

    /// CHECK: Deserialized and validated inside the handler via PriceUpdateV2::try_deserialize.
    /// Must match reserve.config.pyth_oracle.
    pub pyth_price_update: UncheckedAccount<'info>,
}

pub fn refresh_reserve(ctx: Context<RefreshReserve>) -> Result<()> {
    let clock = Clock::get()?;
    let mut reserve = ctx.accounts.reserve.load_mut()?;

    // validate that the passed account matches the oracle configured for this reserve
    require!(
        ctx.accounts.pyth_price_update.key() == reserve.config.pyth_oracle,
        LendingError::InvalidConfig
    );

    // accrue interest up to the current slot before updating the price
    reserve.accrue_interest(clock.slot)?;

    // fetch and validate the Pyth price
    let (price_sf, publish_time) = get_pyth_price(
        ctx.accounts.pyth_price_update.as_ref(),
        MAX_AGE_SECONDS,
        clock.unix_timestamp,
    )?;

    reserve.liquidity.market_price_sf = price_sf;
    reserve.liquidity.price_last_updated_ts = publish_time;

    Ok(())
}
