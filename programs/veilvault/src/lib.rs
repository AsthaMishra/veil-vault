use anchor_lang::prelude::*;

pub mod constants;
pub mod error;
pub mod instructions;
pub mod state;
pub mod utils;

pub use error::*;
use instructions::*;

declare_id!("CMbnY6XXekgVZvFHwmB6yC15TD5x7anD1XmHrm218Wbs");

#[program]
pub mod veilvault {
    use super::*;

    pub fn initialize_market(
        ctx: Context<InitializeMarket>,
        args: InitializeMarketArgs,
    ) -> Result<()> {
        instructions::initialize_market::initialize_market(ctx, args)
    }

    pub fn add_reserve(ctx: Context<AddReserve>, args: AddReserveArgs) -> Result<()> {
        instructions::add_reserve::add_reserve(ctx, args)
    }
}
