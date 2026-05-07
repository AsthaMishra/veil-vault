use anchor_lang::prelude::*;

use crate::state::{InitObligationParams, LendingMarket, Obligation};

#[derive(Accounts)]
pub struct InitObligation<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(
        seeds = [b"lending_market", lending_market.owner.as_ref()],
        bump = lending_market.bump,
    )]
    pub lending_market: Box<Account<'info, LendingMarket>>,

    #[account(
        init,
        payer = owner,
        space = 8 + std::mem::size_of::<Obligation>(),
        seeds = [b"obligation", lending_market.key().as_ref(), owner.key().as_ref()],
        bump,
    )]
    pub obligation: AccountLoader<'info, Obligation>,

    pub system_program: Program<'info, System>,
}

pub fn init_obligation(ctx: Context<InitObligation>) -> Result<()> {
    let mut obligation = ctx.accounts.obligation.load_init()?;
    obligation.init(InitObligationParams {
        bump: ctx.bumps.obligation,
        owner: ctx.accounts.owner.key(),
        lending_market: ctx.accounts.lending_market.key(),
    });
    Ok(())
}
