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

    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        instructions::deposit::deposit(ctx, amount)
    }

    pub fn init_obligation(ctx: Context<InitObligation>) -> Result<()> {
        instructions::init_obligation::init_obligation(ctx)
    }

    pub fn borrow(ctx: Context<Borrow>, amount: u64) -> Result<()> {
        instructions::borrow::borrow(ctx, amount)
    }

    pub fn repay(ctx: Context<Repay>, amount: u64) -> Result<()> {
        instructions::repay::repay(ctx, amount)
    }
}
