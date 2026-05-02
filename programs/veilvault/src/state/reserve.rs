use std::ops::Mul;

use anchor_lang::prelude::*;

use crate::{
    constants::{BPS_SCALER, PERCENT_SCALER, RATE_SCALE, SLOTS_PER_YEAR},
    error::LendingError,
};

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

pub struct ReserveLiquidity {
    pub mint: Pubkey,
    pub supply_vault: Pubkey,
    pub fee_vault: Pubkey,

    pub available_amount: u64,
    pub borrowed_amount_sf: u128,
    pub cumulative_borrow_rate_sf: u128,
}

impl ReserveLiquidity {
    pub fn deposit(&mut self, amount: u64) -> Result<()> {
        self.available_amount = self
            .available_amount
            .checked_add(amount)
            .ok_or(LendingError::MathOverflow)?;
        Ok(())
    }

    pub fn borrow(&mut self, amount: u64) -> Result<()> {
        require!(
            self.available_amount >= amount,
            LendingError::InsufficientLiquidity
        );

        self.available_amount = self
            .available_amount
            .checked_sub(amount)
            .ok_or(LendingError::MathOverflow)?;

        self.borrowed_amount_sf = self
            .borrowed_amount_sf
            .checked_add(amount as u128)
            .ok_or(LendingError::MathOverflow)?;
        Ok(())
    }

    pub fn repay(&mut self, amount: u64) -> Result<()> {
        self.borrowed_amount_sf = self
            .borrowed_amount_sf
            .checked_sub(amount as u128)
            .ok_or(LendingError::MathOverflow)?;

        self.available_amount = self
            .available_amount
            .checked_add(amount)
            .ok_or(LendingError::MathOverflow)?;

        Ok(())
    }

    pub fn withdraw(&mut self, amount: u64) -> Result<()> {
        require!(
            self.available_amount >= amount,
            LendingError::InsufficientLiquidity
        );

        self.available_amount = self
            .available_amount
            .checked_sub(amount)
            .ok_or(LendingError::MathOverflow)?;

        Ok(())
    }

    pub fn accured_interest(&mut self, borrow_rate: u64, slot_elapsed: u64) -> Result<()> {
        require!(slot_elapsed > 0, LendingError::ZeroSlotsElapsed);
        require!(borrow_rate > 0, LendingError::BorrowRateZeroFound);

        let rate = (borrow_rate as u128)
            .checked_mul(slot_elapsed as u128)
            .and_then(|m| m.checked_div(SLOTS_PER_YEAR as u128))
            .and_then(|m| m.checked_div(BPS_SCALER as u128))
            .ok_or(LendingError::MathOverflow)?;

        let debt_accured = (rate as u128)
            .checked_mul(self.borrowed_amount_sf)
            .and_then(|m| m.checked_div(RATE_SCALE))
            .ok_or(LendingError::MathOverflow)?;

        self.borrowed_amount_sf = self
            .borrowed_amount_sf
            .checked_add(debt_accured)
            .and_then(|m| m.checked_div(RATE_SCALE))
            .ok_or(LendingError::MathOverflow)?;

        let rate_accured = (rate as u128)
            .checked_mul(self.cumulative_borrow_rate_sf)
            .and_then(|m| m.checked_div(RATE_SCALE))
            .ok_or(LendingError::MathOverflow)?;

        self.cumulative_borrow_rate_sf = self
            .cumulative_borrow_rate_sf
            .checked_add(rate_accured)
            .ok_or(LendingError::MathOverflow)?;

        Ok(())
    }

    pub fn total_supply(&self) -> Result<u128> {
        (self.available_amount as u128)
            .checked_add(self.borrowed_amount_sf)
            .ok_or(LendingError::MathOverflow.into())
    }

    pub fn utilization_rate(&self) -> Result<u128> {
        let total = self.total_supply().unwrap_or(0);
        if total == 0 {
            return Ok(0);
        }

        (self.borrowed_amount_sf * PERCENT_SCALER)
            .checked_div(total)
            .ok_or(LendingError::MathOverflow.into())
    }
}

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
