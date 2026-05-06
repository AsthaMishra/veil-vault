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

    pub fn health_factor() {}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_pubkey(seed: u8) -> Pubkey {
        Pubkey::new_from_array([seed; 32])
    }

    fn base_obligation() -> Obligation {
        let mut o = Obligation::default();
        o.init(InitObligationParams {
            bump: 1,
            owner: dummy_pubkey(1),
            lending_market: dummy_pubkey(2),
        });
        o
    }

    // --- ObligationCollateral ---

    #[test]
    fn test_collateral_default_is_inactive() {
        let c = ObligationCollateral::default();
        assert!(!c.is_active());
        assert_eq!(c.deposited_amount, 0);
    }

    #[test]
    fn test_collateral_init_is_active() {
        let mut c = ObligationCollateral::default();
        c.init(dummy_pubkey(10));
        assert!(c.is_active());
        assert_eq!(c.deposited_amount, 0);
    }

    // --- ObligationLiquidity ---

    #[test]
    fn test_liquidity_default_is_inactive() {
        let l = ObligationLiquidity::default();
        assert!(!l.is_active());
        assert_eq!(l.borrowed_amount_sf, 0);
        assert_eq!(l.cumulative_borrow_rate_sf, RATE_SCALE);
    }

    #[test]
    fn test_liquidity_init_sets_rate() {
        let mut l = ObligationLiquidity::default();
        l.init(dummy_pubkey(20), RATE_SCALE * 2);
        assert!(l.is_active());
        assert_eq!(l.cumulative_borrow_rate_sf, RATE_SCALE * 2);
    }

    // --- deposit ---

    #[test]
    fn test_deposit_opens_new_slot() {
        let mut o = base_obligation();
        assert!(o.deposit(dummy_pubkey(10), 500).is_ok());
        assert_eq!(o.deposits_count, 1);
        assert_eq!(o.deposits[0].deposited_amount, 500);
    }

    #[test]
    fn test_deposit_adds_to_existing_slot() {
        let mut o = base_obligation();
        let reserve = dummy_pubkey(10);
        o.deposit(reserve, 500).unwrap();
        o.deposit(reserve, 300).unwrap();
        assert_eq!(o.deposits_count, 1);
        assert_eq!(o.deposits[0].deposited_amount, 800);
    }

    #[test]
    fn test_deposit_multiple_reserves() {
        let mut o = base_obligation();
        o.deposit(dummy_pubkey(10), 100).unwrap();
        o.deposit(dummy_pubkey(11), 200).unwrap();
        assert_eq!(o.deposits_count, 2);
    }

    #[test]
    fn test_deposit_zero_fails() {
        let mut o = base_obligation();
        assert!(o.deposit(dummy_pubkey(10), 0).is_err());
    }

    #[test]
    fn test_deposit_slots_full_fails() {
        let mut o = base_obligation();
        for i in 0..MAX_DEPOSITS_COUNT_IN_RESERVE as u8 {
            o.deposit(dummy_pubkey(10 + i), 100).unwrap();
        }
        assert!(o.deposit(dummy_pubkey(99), 100).is_err());
    }

    // --- withdraw ---

    #[test]
    fn test_withdraw_reduces_amount() {
        let mut o = base_obligation();
        let reserve = dummy_pubkey(10);
        o.deposit(reserve, 500).unwrap();
        o.withdraw(reserve, 200).unwrap();
        assert_eq!(o.deposits[0].deposited_amount, 300);
        assert_eq!(o.deposits_count, 1);
    }

    #[test]
    fn test_withdraw_full_clears_slot() {
        let mut o = base_obligation();
        let reserve = dummy_pubkey(10);
        o.deposit(reserve, 500).unwrap();
        o.withdraw(reserve, 500).unwrap();
        assert_eq!(o.deposits_count, 0);
        assert!(!o.deposits[0].is_active());
    }

    #[test]
    fn test_withdraw_more_than_deposited_fails() {
        let mut o = base_obligation();
        let reserve = dummy_pubkey(10);
        o.deposit(reserve, 500).unwrap();
        assert!(o.withdraw(reserve, 600).is_err());
    }

    #[test]
    fn test_withdraw_unknown_reserve_fails() {
        let mut o = base_obligation();
        assert!(o.withdraw(dummy_pubkey(99), 100).is_err());
    }

    // --- borrow ---

    #[test]
    fn test_borrow_opens_new_slot() {
        let mut o = base_obligation();
        assert!(o.borrow(dummy_pubkey(20), 100, RATE_SCALE).is_ok());
        assert_eq!(o.borrows_count, 1);
        assert_eq!(o.borrows[0].borrowed_amount_sf, 100 * RATE_SCALE);
    }

    #[test]
    fn test_borrow_adds_to_existing_slot() {
        let mut o = base_obligation();
        let reserve = dummy_pubkey(20);
        o.borrow(reserve, 100, RATE_SCALE).unwrap();
        o.borrow(reserve, 50, RATE_SCALE).unwrap();
        assert_eq!(o.borrows_count, 1);
        assert_eq!(o.borrows[0].borrowed_amount_sf, 150 * RATE_SCALE);
    }

    #[test]
    fn test_borrow_zero_fails() {
        let mut o = base_obligation();
        assert!(o.borrow(dummy_pubkey(20), 0, RATE_SCALE).is_err());
    }

    #[test]
    fn test_borrow_slots_full_fails() {
        let mut o = base_obligation();
        for i in 0..MAX_BORROW_COUNT_IN_RESERVE as u8 {
            o.borrow(dummy_pubkey(20 + i), 10, RATE_SCALE).unwrap();
        }
        assert!(o.borrow(dummy_pubkey(99), 10, RATE_SCALE).is_err());
    }

    // --- repay ---

    #[test]
    fn test_repay_reduces_debt() {
        let mut o = base_obligation();
        let reserve = dummy_pubkey(20);
        o.borrow(reserve, 100, RATE_SCALE).unwrap();
        o.repay(reserve, 40).unwrap();
        assert_eq!(o.borrows[0].borrowed_amount_sf, 60 * RATE_SCALE);
        assert_eq!(o.borrows_count, 1);
    }

    #[test]
    fn test_repay_full_clears_slot() {
        let mut o = base_obligation();
        let reserve = dummy_pubkey(20);
        o.borrow(reserve, 100, RATE_SCALE).unwrap();
        o.repay(reserve, 100).unwrap();
        assert_eq!(o.borrows_count, 0);
        assert!(!o.borrows[0].is_active());
    }

    #[test]
    fn test_repay_more_than_owed_fails() {
        let mut o = base_obligation();
        let reserve = dummy_pubkey(20);
        o.borrow(reserve, 100, RATE_SCALE).unwrap();
        assert!(o.repay(reserve, 101).is_err());
    }

    #[test]
    fn test_repay_unknown_reserve_fails() {
        let mut o = base_obligation();
        assert!(o.repay(dummy_pubkey(99), 50).is_err());
    }

    // --- accrue_interest ---

    #[test]
    fn test_accrue_interest_grows_debt() {
        let mut o = base_obligation();
        let reserve = dummy_pubkey(20);
        o.borrow(reserve, 100, RATE_SCALE).unwrap();

        // rate doubles
        o.accrue_interest(0, RATE_SCALE * 2).unwrap();

        assert_eq!(o.borrows[0].borrowed_amount_sf, 200 * RATE_SCALE);
        assert_eq!(o.borrows[0].cumulative_borrow_rate_sf, RATE_SCALE * 2);
    }

    #[test]
    fn test_accrue_interest_no_change_when_rate_unchanged() {
        let mut o = base_obligation();
        let reserve = dummy_pubkey(20);
        o.borrow(reserve, 100, RATE_SCALE).unwrap();
        o.accrue_interest(0, RATE_SCALE).unwrap();
        assert_eq!(o.borrows[0].borrowed_amount_sf, 100 * RATE_SCALE);
    }

    #[test]
    fn test_accrue_interest_rate_decrease_fails() {
        let mut o = base_obligation();
        o.borrow(dummy_pubkey(20), 100, RATE_SCALE * 2).unwrap();
        assert!(o.accrue_interest(0, RATE_SCALE).is_err());
    }

    #[test]
    fn test_accrue_interest_inactive_slot_is_noop() {
        let mut o = base_obligation();
        assert!(o.accrue_interest(0, RATE_SCALE * 2).is_ok());
        assert_eq!(o.borrows[0].borrowed_amount_sf, 0);
    }

    // --- compounding ---

    #[test]
    fn test_accrue_interest_compounds_across_refreshes() {
        let mut o = base_obligation();
        let reserve = dummy_pubkey(20);
        o.borrow(reserve, 100, RATE_SCALE).unwrap();

        // first refresh: rate goes 1.0 → 1.05
        o.accrue_interest(0, RATE_SCALE * 105 / 100).unwrap();
        // second refresh: rate goes 1.05 → 1.1025
        o.accrue_interest(0, RATE_SCALE * 11025 / 10000).unwrap();

        // 100 × 1.1025 = 110.25 → 110 * RATE_SCALE (integer division)
        let expected = 100 * RATE_SCALE * 11025 / 10000;
        assert_eq!(o.borrows[0].borrowed_amount_sf, expected);
    }
}
