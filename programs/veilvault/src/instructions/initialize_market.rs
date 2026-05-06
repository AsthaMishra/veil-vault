use anchor_lang::prelude::*;

use crate::{
    error::LendingError,
    state::{InitializeLendingParams, LendingMarket},
};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct InitializeMarketArgs {
    pub quote_currency: [u8; 32],
    pub protocol_fee_bps: u16,
}

#[derive(Accounts)]
pub struct InitializeMarket<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(
        init,
        payer = owner,
        space = 8 + std::mem::size_of::<LendingMarket>(),
        seeds = [b"lending_market", owner.key().as_ref()],
        bump,
    )]
    pub lending_market: Account<'info, LendingMarket>,

    pub system_program: Program<'info, System>,
}

pub fn initialize_market(ctx: Context<InitializeMarket>, args: InitializeMarketArgs) -> Result<()> {
    require!(
        args.protocol_fee_bps <= crate::constants::MAX_PROTOCOL_FEE_BPS,
        LendingError::InvalidFee
    );

    ctx.accounts.lending_market.init(InitializeLendingParams {
        bump: ctx.bumps.lending_market,
        owner: ctx.accounts.owner.key(),
        quote_currency: args.quote_currency,
        protocol_fee_bps: args.protocol_fee_bps,
    })?;

    Ok(())
}
