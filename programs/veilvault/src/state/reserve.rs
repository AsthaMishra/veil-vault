use anchor_lang::prelude::*;

use crate::error::LendingError;

#[account]
pub struct Reserve {
    pub config: ReserveConfig,
}

impl Reserve {
    pub fn new(config: ReserveConfig) -> Self {
        Self { config }
    }
}

#[account]
pub struct ReserveConfig {}

#[account]
pub struct ReserveLiquidity {}

// #[account]
#[derive(Debug)]
#[zero_copy]
#[repr(C)]
pub struct ReserveCollateral {
    // mint and burn factory
    pub mint_pda: Pubkey,

    //cached here
    pub mint_total_supply: u64,

    pub supply_vault_pda: Pubkey,

    pub padding1: [u64; 64],
    pub padding2: [u64; 64],
}

impl ReserveCollateral {
    pub fn new(mint_pda: Pubkey, mint_total_supply: u64, supply_vault_pda: Pubkey) -> Self {
        Self {
            mint_pda,
            mint_total_supply,
            supply_vault_pda,
            padding1: [0; 64],
            padding2: [0; 64],
        }
    }
    pub fn mint(&mut self, collateral_amount: u64) -> Result<()> {
        self.mint_total_supply = self
            .mint_total_supply
            .checked_add(collateral_amount)
            .ok_or(LendingError::MathOverflow)?;
        Ok(())
    }

    pub fn burn(&mut self, collateral_amount: u64) -> Result<()> {
        self.mint_total_supply = self
            .mint_total_supply
            .checked_sub(collateral_amount)
            .ok_or(LendingError::MathOverflow)?;
        Ok(())
    }

    pub fn exchange_rate() {}
}

pub struct CollateralExchangeRate {
    // pub collateral: u128,
    // pub liquidity: Fraction,
}

impl CollateralExchangeRate {}
