use anchor_lang::prelude::*;

use crate::state::LendingMarket;

#[derive(Accounts)]
pub struct UpdateMarketAuthority<'info> {
    pub owner: Signer<'info>,

    // The PDA is seeded from the current owner's pubkey, so only the current owner
    // can pass the seed constraint. After this instruction completes, the market's
    // stored owner changes — but all other instructions derive the market PDA from
    // [b"lending_market", new_owner_signer], which will NOT match this PDA's address.
    // This is a known limitation of the current seed design (see CONTEXT.md).
    // A production system would use a stable seed independent of the authority key.
    #[account(
        mut,
        seeds = [b"lending_market", owner.key().as_ref()],
        bump = lending_market.bump,
        has_one = owner,
    )]
    pub lending_market: Box<Account<'info, LendingMarket>>,
}

pub fn update_market_authority(
    ctx: Context<UpdateMarketAuthority>,
    new_authority: Pubkey,
) -> Result<()> {
    ctx.accounts.lending_market.owner = new_authority;
    Ok(())
}
