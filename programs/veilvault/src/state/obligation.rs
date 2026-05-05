use anchor_lang::prelude::*;

use crate::{
    constants::{MAX_BORROW_COUNT_IN_RESERVE, MAX_DEPOSITS_COUNT_IN_RESERVE, RATE_SCALE},
    error::LendingError,
    utils::LastUpdate,
};

#[derive(Debug)]
#[zero_copy]
#[repr(C)]
pub struct ObligationCollateral {
    pub deposit_reserve: Pubkey,
    pub deposited_amount: u64, // in cTokens
}

impl Default for ObligationCollateral {
    fn default() -> Self {
        Self {
            deposit_reserve: Pubkey::default(),
            deposited_amount: 0,
        }
    }
}

impl ObligationCollateral {
    pub fn init(&mut self, deposit_reserve: Pubkey) {
        *self = Self::default();
        self.deposit_reserve = deposit_reserve;
    }

    pub fn is_active(&self) -> bool {
        self.deposit_reserve != Pubkey::default()
    }
}

#[derive(Debug)]
#[zero_copy]
#[repr(C)]
pub struct ObligationLiquidity {
    pub borrow_reserve: Pubkey,
    pub borrowed_amount_sf: u128,
    // snapshotted from reserve at borrow time; ratio to current rate gives accrued interest
    pub cumulative_borrow_rate_sf: u128,
}

impl Default for ObligationLiquidity {
    fn default() -> Self {
        Self {
            borrow_reserve: Pubkey::default(),
            borrowed_amount_sf: 0,
            cumulative_borrow_rate_sf: RATE_SCALE, // 1.0 — no interest at init
        }
    }
}

impl ObligationLiquidity {
    pub fn init(&mut self, borrow_reserve: Pubkey, cumulative_borrow_rate_sf: u128) {
        *self = Self::default();
        self.borrow_reserve = borrow_reserve;
        self.cumulative_borrow_rate_sf = cumulative_borrow_rate_sf;
    }

    pub fn is_active(&self) -> bool {
        self.borrow_reserve != Pubkey::default()
    }
}

pub struct InitObligationParams {
    pub bump: u8,
    pub owner: Pubkey,
    pub lending_market: Pubkey,
}

#[derive(Debug)]
#[zero_copy]
#[repr(C)]
pub struct Obligation {
    pub lending_market: Pubkey,
    pub owner: Pubkey,
    pub last_update: LastUpdate,
    pub deposits: [ObligationCollateral; MAX_DEPOSITS_COUNT_IN_RESERVE],
    pub borrows: [ObligationLiquidity; MAX_BORROW_COUNT_IN_RESERVE],
    pub deposits_count: u8,
    pub borrows_count: u8,
    pub bump: u8,
    pub padding: [u8; 13],
    pub padding1: [u64; 64],
}

impl Default for Obligation {
    fn default() -> Self {
        Self {
            lending_market: Pubkey::default(),
            owner: Pubkey::default(),
            bump: 0,
            last_update: LastUpdate::default(),
            deposits: std::array::from_fn(|_| ObligationCollateral::default()),
            borrows: std::array::from_fn(|_| ObligationLiquidity::default()),
            deposits_count: 0,
            borrows_count: 0,
            padding: [0; 13],
            padding1: [0; 64],
        }
    }
}

impl Obligation {
    pub fn init(&mut self, params: InitObligationParams) {
        *self = Self::default();
        self.bump = params.bump;
        self.owner = params.owner;
        self.lending_market = params.lending_market;
    }

    pub fn deposit(&mut self, reserve: Pubkey, amount: u64) -> Result<()> {
        require!(amount > 0, LendingError::InvalidAmount);

        let slot = self.find_or_add_deposit(reserve)?;

        if !self.deposits[slot].is_active() {
            self.deposits[slot].init(reserve);
            self.deposits_count = self.deposits_count.saturating_add(1);
        }

        self.deposits[slot].deposited_amount = self.deposits[slot]
            .deposited_amount
            .checked_add(amount)
            .ok_or(LendingError::MathOverflow)?;

        Ok(())
    }

    pub fn withdraw(&mut self, reserve: Pubkey, amount: u64) -> Result<()> {
        require!(amount > 0, LendingError::InvalidAmount);

        let slot = self.find_deposit(reserve)?;

        require!(
            amount <= self.deposits[slot].deposited_amount,
            LendingError::InsufficientCollateral
        );

        self.deposits[slot].deposited_amount = self.deposits[slot]
            .deposited_amount
            .checked_sub(amount)
            .ok_or(LendingError::MathOverflow)?;

        if self.deposits[slot].deposited_amount == 0 {
            self.deposits[slot] = ObligationCollateral::default();
            self.deposits_count = self.deposits_count.saturating_sub(1);
        }

        Ok(())
    }

    pub fn borrow(
        &mut self,
        reserve: Pubkey,
        amount: u128,
        cumulative_borrow_rate_sf: u128,
    ) -> Result<()> {
        require!(amount > 0, LendingError::InvalidAmount);

        let slot = self.find_or_add_borrow(reserve)?;

        if !self.borrows[slot].is_active() {
            self.borrows[slot].init(reserve, cumulative_borrow_rate_sf);
            self.borrows_count = self.borrows_count.saturating_add(1);
        }

        let amount_sf = amount
            .checked_mul(RATE_SCALE)
            .ok_or(LendingError::MathOverflow)?;

        self.borrows[slot].borrowed_amount_sf = self.borrows[slot]
            .borrowed_amount_sf
            .checked_add(amount_sf)
            .ok_or(LendingError::MathOverflow)?;

        Ok(())
    }

    pub fn repay(&mut self, reserve: Pubkey, amount: u128) -> Result<()> {
        require!(amount > 0, LendingError::InvalidAmount);

        let slot = self.find_borrow(reserve)?;

        let amount_sf = amount
            .checked_mul(RATE_SCALE)
            .ok_or(LendingError::MathOverflow)?;

        require!(
            amount_sf <= self.borrows[slot].borrowed_amount_sf,
            LendingError::InvalidAmount
        );

        self.borrows[slot].borrowed_amount_sf = self.borrows[slot]
            .borrowed_amount_sf
            .checked_sub(amount_sf)
            .ok_or(LendingError::MathOverflow)?;

        if self.borrows[slot].borrowed_amount_sf == 0 {
            self.borrows[slot] = ObligationLiquidity::default();
            self.borrows_count = self.borrows_count.saturating_sub(1);
        }

        Ok(())
    }

    pub fn accrue_interest(
        &mut self,
        slot_index: usize,
        current_cumulative_rate_sf: u128,
    ) -> Result<()> {
        let slot = &mut self.borrows[slot_index];

        if !slot.is_active() {
            return Ok(());
        }

        require!(
            current_cumulative_rate_sf >= slot.cumulative_borrow_rate_sf,
            LendingError::BorrowRateDecreased
        );

        // no rate change since last refresh — nothing to accrue
        if current_cumulative_rate_sf == slot.cumulative_borrow_rate_sf {
            return Ok(());
        }

        slot.borrowed_amount_sf = slot
            .borrowed_amount_sf
            .checked_mul(current_cumulative_rate_sf)
            .ok_or(LendingError::MathOverflow)?
            .checked_div(slot.cumulative_borrow_rate_sf)
            .ok_or(LendingError::MathOverflow)?;

        slot.cumulative_borrow_rate_sf = current_cumulative_rate_sf;

        Ok(())
    }

    pub fn is_healthy() {}

    pub fn find_or_add_deposit(&self, reserve: Pubkey) -> Result<usize> {
        if let Some(slot) = self
            .deposits
            .iter()
            .position(|c| c.deposit_reserve == reserve)
        {
            return Ok(slot);
        }

        if let Some(slot) = self.deposits.iter().position(|c| !c.is_active()) {
            return Ok(slot);
        }

        err!(LendingError::ObligationDepositsFull)
    }

    pub fn find_or_add_borrow(&self, reserve: Pubkey) -> Result<usize> {
        if let Some(slot) = self
            .borrows
            .iter()
            .position(|c| c.borrow_reserve == reserve)
        {
            return Ok(slot);
        }

        // fixed: search borrows not deposits
        if let Some(slot) = self.borrows.iter().position(|c| !c.is_active()) {
            return Ok(slot);
        }

        err!(LendingError::ObligationBorrowsFull)
    }

    pub fn find_deposit(&self, reserve: Pubkey) -> Result<usize> {
        self.deposits
            .iter()
            .position(|c| c.deposit_reserve == reserve)
            .ok_or(LendingError::DepositNotFound.into())
    }

    pub fn find_borrow(&self, reserve: Pubkey) -> Result<usize> {
        self.borrows
            .iter()
            .position(|c| c.borrow_reserve == reserve)
            .ok_or(LendingError::BorrowNotFound.into())
    }

    pub fn active_deposits(&self) -> u8 {
        self.deposits_count
    }
    pub fn active_borrow(&self) -> u8 {
        self.borrows_count
    }
}
