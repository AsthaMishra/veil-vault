use anchor_lang::prelude::*;

pub mod state;
use state::*;

pub mod error;
use error::*;

pub mod constants;
use constants::*;

declare_id!("CMbnY6XXekgVZvFHwmB6yC15TD5x7anD1XmHrm218Wbs");

#[program]
pub mod veilvault {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        msg!("Greetings from: {:?}", ctx.program_id);

       
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize {}
