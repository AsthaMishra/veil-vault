use anchor_lang::prelude::*;

use crate::state::LendingMarket;

#[derive(Accounts)]
pub struct SetPause<'info> {
    pub owner: Signer<'info>,

    #[account(
        mut,
        seeds = [b"lending_market", owner.key().as_ref()],
        bump = lending_market.bump,
        has_one = owner,
    )]
    pub lending_market: Box<Account<'info, LendingMarket>>,
}

pub fn set_pause(ctx: Context<SetPause>, paused: bool) -> Result<()> {
    ctx.accounts.lending_market.set_emergency_pause(paused)
}
