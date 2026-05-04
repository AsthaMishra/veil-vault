use std::ops::Mul;

use anchor_lang::prelude::*;

use crate::{
    constants::{
        BPS_SCALER, MAX_LIQUIDATION_THRESHOLD_PCT, MAX_PROTOCOL_FEE_BPS, MAX_UTILIZATION_BPS,
        PERCENT_SCALER, RATE_SCALE, SLOTS_PER_YEAR,
    },
    error::LendingError,
};

pub const RESERVE_VERSION: u64 = 1;

#[account]
#[derive(Default)]
pub struct Reserve {
    pub version: u64,
    pub last_update_slot: u64,
    pub lending_market: Pubkey,
    pub config: ReserveConfig,
    pub liquidity: ReserveLiquidity,
    pub collateral: ReserveCollateral,
}

pub struct InitReserveParams {
    pub current_slot: u64,
    pub lending_market: Pubkey,
    pub liquidity: ReserveLiquidity,
    pub collateral: ReserveCollateral,
    pub config: ReserveConfig,
}

impl Reserve {
    pub fn init(&mut self, params: InitReserveParams) {
        *self = Self::default();
        self.version = RESERVE_VERSION;
        self.last_update_slot = params.current_slot;
        self.lending_market = params.lending_market;
        self.liquidity = params.liquidity;
        self.collateral = params.collateral;
        self.config = params.config;
    }

    pub fn collateral_exchange_rate(&self) -> CollateralExchangeRate {
        self.collateral
            .exchange_rate(self.liquidity.total_supply().unwrap_or(0))
    }

    pub fn utilization_rate(&self) -> Result<u128> {
        self.liquidity.utilization_rate()
    }

    // utilization_rate() returns 0-100 (pct), borrow_rate() expects 0-10000 (bps)
    pub fn current_borrow_rate(&self) -> Result<u16> {
        let utilization_bps = self.liquidity.utilization_rate()? * 100;
        self.config.borrow_rate(utilization_bps as u16)
    }

    pub fn deposit_limit_crossed(&self) -> bool {
        self.liquidity.total_supply().unwrap_or(0) >= self.config.deposit_limit as u128
    }

    pub fn borrow_limit_crossed(&self) -> bool {
        self.liquidity.borrowed_amount_sf >= self.config.borrow_limit as u128
    }

    // Deposits liquidity, mints collateral. Returns collateral amount minted.
    pub fn deposit_liquidity(&mut self, amount: u64) -> Result<u64> {
        require!(
            !self.deposit_limit_crossed(),
            LendingError::DepositLimitExceeded
        );

        let exchange_rate = self.collateral_exchange_rate();
        let collateral_amount = exchange_rate.liquidity_to_collateral(amount)?;

        self.liquidity.deposit(amount)?;
        self.collateral.mint(collateral_amount)?;

        Ok(collateral_amount)
    }

    // Burns collateral, withdraws liquidity. Returns liquidity amount returned to user.
    pub fn redeem_collateral(&mut self, collateral_amount: u64) -> Result<u64> {
        let exchange_rate = self.collateral_exchange_rate();
        let liquidity_amount = exchange_rate.collateral_to_liquidity(collateral_amount)?;

        self.collateral.burn(collateral_amount)?;
        self.liquidity.withdraw(liquidity_amount)?;

        Ok(liquidity_amount)
    }

    pub fn borrow(&mut self, amount: u64) -> Result<()> {
        require!(
            !self.borrow_limit_crossed(),
            LendingError::BorrowLimitExceeded
        );
        self.liquidity.borrow(amount)
    }

    pub fn repay(&mut self, amount: u64) -> Result<()> {
        self.liquidity.repay(amount)
    }

    pub fn accrue_interest(&mut self, current_slot: u64) -> Result<()> {
        let slots_elapsed = current_slot.saturating_sub(self.last_update_slot);
        if slots_elapsed == 0 {
            return Ok(());
        }

        let borrow_rate = self.current_borrow_rate()?;
        if borrow_rate > 0 {
            self.liquidity
                .accured_interest(borrow_rate as u64, slots_elapsed)?;
        }

        self.last_update_slot = current_slot;
        Ok(())
    }
}

#[account]
#[derive(Default)]
pub struct ReserveConfig {
    pub status: u8, // 0=active, 1=frozen, 2=deprecated

    pub min_borrow_rate_bps: u16,
    pub optimal_borrow_rate_bps: u16,
    pub max_borrow_rate_bps: u16,
    pub optimal_utilization_bps: u16,

    pub loan_to_value_pct: u8,         // borrow upto cretain percentage
    pub liquidation_threshold_pct: u8, // liquidatoion will strat after this percentage
    pub liquidation_bonus_pct: u16,    // 5% discount for liquidators

    pub deposit_limit: u64,
    pub borrow_limit: u64,

    pub protocol_fee: u16,
}

pub struct InitReserveConfigParams {
    pub status: u8,
    pub min_borrow_rate_bps: u16,
    pub optimal_borrow_rate_bps: u16,
    pub max_borrow_rate_bps: u16,
    pub optimal_utilization_bps: u16,
    pub loan_to_value_pct: u8,
    pub liquidation_threshold_pct: u8,
    pub liquidation_bonus_pct: u16,
    pub deposit_limit: u64,
    pub borrow_limit: u64,
    pub protocol_fee: u16,
}

impl ReserveConfig {
    pub fn init(&mut self, params: InitReserveConfigParams) -> Result<()> {
        *self = Self::default();
        self.status = params.status;
        self.min_borrow_rate_bps = params.min_borrow_rate_bps;
        self.optimal_borrow_rate_bps = params.optimal_borrow_rate_bps;
        self.max_borrow_rate_bps = params.max_borrow_rate_bps;
        self.optimal_utilization_bps = params.optimal_utilization_bps;
        self.loan_to_value_pct = params.loan_to_value_pct;
        self.liquidation_threshold_pct = params.liquidation_threshold_pct;
        self.liquidation_bonus_pct = params.liquidation_bonus_pct;
        self.deposit_limit = params.deposit_limit;
        self.borrow_limit = params.borrow_limit;
        self.protocol_fee = params.protocol_fee;
        self.validate()?;
        Ok(())
    }

    // Replaces the current config after validating the incoming one.
    // Called from governance/admin instructions that update reserve params.
    pub fn update(&mut self, new_config: ReserveConfig) -> Result<()> {
        new_config.validate()?;
        *self = new_config;
        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.status == 0
    }

    pub fn is_frozen(&self) -> bool {
        self.status == 1
    }

    // Returns borrow rate in bps given current utilization in bps (0–10000)
    // Implements the two-segment kinked curve:
    //   segment 1: 0% → optimal_utilization  (gentle slope)
    //   segment 2: optimal_utilization → 100% (steep slope)
    pub fn borrow_rate(&self, utilization_bps: u16) -> Result<u16> {
        let util = utilization_bps as u64;
        let optimal_util = self.optimal_utilization_bps as u64;

        let rate: u64 = if util <= optimal_util {
            // segment 1: min_rate + utilization * (optimal_rate - min_rate) / optimal_util
            let min: u64 = self.min_borrow_rate_bps as u64;
            let optimal: u64 = self.optimal_borrow_rate_bps as u64;
            let slope_numerator: u64 = util
                .checked_mul(optimal.saturating_sub(min))
                .ok_or(LendingError::MathOverflow)?;
            let slope = if optimal_util == 0 {
                0
            } else {
                slope_numerator
                    .checked_div(optimal_util)
                    .ok_or(LendingError::MathOverflow)?
            };
            min.checked_add(slope).ok_or(LendingError::MathOverflow)?
        } else {
            // segment 2: optimal_rate + (util - optimal_util) * (max_rate - optimal_rate) / (10000 - optimal_util)
            let optimal = self.optimal_borrow_rate_bps as u64;
            let max: u64 = self.max_borrow_rate_bps as u64;
            let excess_util = util.saturating_sub(optimal_util);
            let remaining_util = (BPS_SCALER as u64).saturating_sub(optimal_util);
            let slope_numerator = excess_util
                .checked_mul(max.saturating_sub(optimal))
                .ok_or(LendingError::MathOverflow)?;
            let slope = if remaining_util == 0 {
                0
            } else {
                slope_numerator
                    .checked_div(remaining_util)
                    .ok_or(LendingError::MathOverflow)?
            };
            optimal
                .checked_add(slope)
                .ok_or(LendingError::MathOverflow)?
        };

        Ok(rate as u16)
    }

    pub fn validate(&self) -> Result<()> {
        require!(
            self.loan_to_value_pct < self.liquidation_threshold_pct,
            LendingError::InvalidConfig
        );
        require!(
            self.liquidation_threshold_pct <= MAX_LIQUIDATION_THRESHOLD_PCT,
            LendingError::InvalidConfig
        );
        require!(
            self.min_borrow_rate_bps <= self.optimal_borrow_rate_bps
                && self.optimal_borrow_rate_bps <= self.max_borrow_rate_bps,
            LendingError::InvalidConfig
        );
        require!(
            self.optimal_utilization_bps <= MAX_UTILIZATION_BPS as u16,
            LendingError::InvalidConfig
        );
        require!(
            self.protocol_fee <= MAX_PROTOCOL_FEE_BPS,
            LendingError::InvalidConfig
        );
        Ok(())
    }
}

pub struct NewReserveLiquidityParams {
    pub mint: Pubkey,
    pub supply_vault: Pubkey,
    pub fee_vault: Pubkey,
}

#[account]
pub struct ReserveLiquidity {
    pub mint: Pubkey,
    pub supply_vault: Pubkey,
    pub fee_vault: Pubkey,

    pub available_amount: u64,
    pub borrowed_amount_sf: u128,
    pub cumulative_borrow_rate_sf: u128,
}

impl Default for ReserveLiquidity {
    fn default() -> Self {
        Self {
            mint: Pubkey::default(),
            supply_vault: Pubkey::default(),
            fee_vault: Pubkey::default(),
            available_amount: 0,
            borrowed_amount_sf: 0,
            cumulative_borrow_rate_sf: RATE_SCALE, // 1.0 — no interest accrued yet
        }
    }
}

impl ReserveLiquidity {
    pub fn init(&mut self, params: NewReserveLiquidityParams) {
        *self = Self::default();
        self.mint = params.mint;
        self.supply_vault = params.supply_vault;
        self.fee_vault = params.fee_vault;
    }

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

        // multiply by RATE_SCALE before dividing to preserve precision for small slot counts
        let rate = (borrow_rate as u128)
            .checked_mul(slot_elapsed as u128)
            .and_then(|m| m.checked_mul(RATE_SCALE))
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

pub struct NewReserveCollateralParams {
    pub mint_pda: Pubkey,
    pub supply_vault_pda: Pubkey,
}

#[account]
#[derive(Debug)]
pub struct ReserveCollateral {
    pub mint_pda: Pubkey,
    pub mint_total_supply: u64,
    pub supply_vault_pda: Pubkey,
    pub padding1: [u64; 64],
    pub padding2: [u64; 64],
}

impl Default for ReserveCollateral {
    fn default() -> Self {
        Self {
            mint_pda: Pubkey::default(),
            mint_total_supply: 0,
            supply_vault_pda: Pubkey::default(),
            padding1: [0; 64],
            padding2: [0; 64],
        }
    }
}

impl ReserveCollateral {
    pub fn init(&mut self, params: NewReserveCollateralParams) {
        *self = Self::default();
        self.mint_pda = params.mint_pda;
        self.supply_vault_pda = params.supply_vault_pda;
    }
    pub fn mint(&mut self, collateral_amount: u64) -> Result<()> {
        self.mint_total_supply = self
            .mint_total_supply
            .checked_add(collateral_amount)
            .ok_or(LendingError::MathOverflow)?;
        Ok(())
    }

    pub fn burn(&mut self, collateral_amount: u64) -> Result<()> {
        require!(
            self.mint_total_supply >= collateral_amount,
            LendingError::InsufficientCollateral
        );
        self.mint_total_supply = self
            .mint_total_supply
            .checked_sub(collateral_amount)
            .ok_or(LendingError::MathOverflow)?;
        Ok(())
    }

    // total_liquidity comes from ReserveLiquidity::total_supply()
    pub fn exchange_rate(&self, total_liquidity: u128) -> CollateralExchangeRate {
        if self.mint_total_supply == 0 || total_liquidity == 0 {
            // 1:1 initial rate — pool is empty, first depositor sets the baseline
            CollateralExchangeRate {
                collateral_supply: 1,
                total_liquidity: 1,
            }
        } else {
            CollateralExchangeRate {
                collateral_supply: self.mint_total_supply as u128,
                total_liquidity,
            }
        }
    }
}

pub struct CollateralExchangeRate {
    collateral_supply: u128,
    total_liquidity: u128,
}

impl CollateralExchangeRate {
    // burn cTokens → get back underlying (withdraw)
    // floor: user gets slightly less, protocol never overpays
    pub fn collateral_to_liquidity(&self, collateral_amount: u64) -> Result<u64> {
        let result = (collateral_amount as u128)
            .checked_mul(self.total_liquidity)
            .and_then(|n| n.checked_div(self.collateral_supply))
            .ok_or(LendingError::MathOverflow)?;
        Ok(result as u64)
    }

    // deposit underlying → mint cTokens
    // floor: user gets slightly fewer cTokens
    pub fn liquidity_to_collateral(&self, liquidity_amount: u64) -> Result<u64> {
        let result = (liquidity_amount as u128)
            .checked_mul(self.collateral_supply)
            .and_then(|n| n.checked_div(self.total_liquidity))
            .ok_or(LendingError::MathOverflow)?;
        Ok(result as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> ReserveConfig {
        ReserveConfig {
            status: 0,
            min_borrow_rate_bps: 200,      // 2%
            optimal_borrow_rate_bps: 2000, // 20%
            max_borrow_rate_bps: 10000,    // 100%
            optimal_utilization_bps: 8000, // 80%
            loan_to_value_pct: 75,
            liquidation_threshold_pct: 80,
            liquidation_bonus_pct: 500,
            deposit_limit: 1_000_000,
            borrow_limit: 800_000,
            protocol_fee: 500,
        }
    }

    // --- validate ---

    #[test]
    fn test_validate_valid_config() {
        assert!(base_config().validate().is_ok());
    }

    #[test]
    fn test_validate_ltv_must_be_below_liquidation_threshold() {
        let mut config = base_config();
        config.loan_to_value_pct = 80; // equal to threshold — must be strictly less
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_borrow_rates_must_be_ordered() {
        let mut config = base_config();
        config.optimal_borrow_rate_bps = 100; // below min_borrow_rate
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_protocol_fee_exceeds_max() {
        let mut config = base_config();
        config.protocol_fee = MAX_PROTOCOL_FEE_BPS + 1;
        assert!(config.validate().is_err());
    }

    // --- borrow_rate ---

    #[test]
    fn test_borrow_rate_at_zero_utilization() {
        // at 0% util → should return min_borrow_rate exactly
        let rate = base_config().borrow_rate(0).unwrap();
        assert_eq!(rate, 200);
    }

    #[test]
    fn test_borrow_rate_at_optimal_utilization() {
        // at kink (80%) → should return optimal_borrow_rate exactly
        let rate = base_config().borrow_rate(8000).unwrap();
        assert_eq!(rate, 2000);
    }

    #[test]
    fn test_borrow_rate_at_max_utilization() {
        // at 100% util → should return max_borrow_rate exactly
        let rate = base_config().borrow_rate(10000).unwrap();
        assert_eq!(rate, 10000);
    }

    #[test]
    fn test_borrow_rate_midpoint_segment1() {
        // at 40% util (halfway through segment 1) → halfway between min and optimal
        // rate = 200 + 4000 * (2000 - 200) / 8000 = 200 + 900 = 1100
        let rate = base_config().borrow_rate(4000).unwrap();
        assert_eq!(rate, 1100);
    }

    #[test]
    fn test_borrow_rate_midpoint_segment2() {
        // at 90% util (halfway through segment 2) → halfway between optimal and max
        // rate = 2000 + (9000-8000) * (10000-2000) / (10000-8000) = 2000 + 4000 = 6000
        let rate = base_config().borrow_rate(9000).unwrap();
        assert_eq!(rate, 6000);
    }

    // --- CollateralExchangeRate ---

    #[test]
    fn test_exchange_rate_one_to_one() {
        // 100 cTokens backed by 100 liquidity → 1:1
        let rate = CollateralExchangeRate {
            collateral_supply: 100,
            total_liquidity: 100,
        };
        assert_eq!(rate.collateral_to_liquidity(50).unwrap(), 50);
        assert_eq!(rate.liquidity_to_collateral(50).unwrap(), 50);
    }

    #[test]
    fn test_exchange_rate_grown() {
        // 100 cTokens backed by 108 liquidity (interest accrued)
        // redeem 50 cTokens → should get 54 USDC back
        let rate = CollateralExchangeRate {
            collateral_supply: 100,
            total_liquidity: 108,
        };
        assert_eq!(rate.collateral_to_liquidity(50).unwrap(), 54);
    }

    #[test]
    fn test_exchange_rate_deposit_mints_fewer_ctokens() {
        // 100 cTokens backed by 108 liquidity
        // deposit 108 → should mint 100 cTokens (not 108)
        let rate = CollateralExchangeRate {
            collateral_supply: 100,
            total_liquidity: 108,
        };
        assert_eq!(rate.liquidity_to_collateral(108).unwrap(), 100);
    }

    #[test]
    fn test_empty_pool_returns_initial_rate() {
        let collateral = ReserveCollateral {
            mint_pda: Pubkey::default(),
            mint_total_supply: 0,
            supply_vault_pda: Pubkey::default(),
            padding1: [0; 64],
            padding2: [0; 64],
        };
        let rate = collateral.exchange_rate(0);
        // initial 1:1 rate: 50 cTokens → 50 liquidity
        assert_eq!(rate.collateral_to_liquidity(50).unwrap(), 50);
    }

    // --- ReserveLiquidity ---

    fn base_liquidity() -> ReserveLiquidity {
        ReserveLiquidity {
            mint: Pubkey::default(),
            supply_vault: Pubkey::default(),
            fee_vault: Pubkey::default(),
            available_amount: 0,
            borrowed_amount_sf: 0,
            cumulative_borrow_rate_sf: RATE_SCALE,
        }
    }

    #[test]
    fn test_deposit_increases_available() {
        let mut liq = base_liquidity();
        liq.deposit(1000).unwrap();
        assert_eq!(liq.available_amount, 1000);
    }

    #[test]
    fn test_borrow_moves_amount_to_borrowed() {
        let mut liq = base_liquidity();
        liq.deposit(1000).unwrap();
        liq.borrow(800).unwrap();
        assert_eq!(liq.available_amount, 200);
        assert_eq!(liq.borrowed_amount_sf, 800);
    }

    #[test]
    fn test_borrow_fails_when_insufficient_liquidity() {
        let mut liq = base_liquidity();
        liq.deposit(500).unwrap();
        assert!(liq.borrow(600).is_err());
    }

    #[test]
    fn test_repay_restores_available() {
        let mut liq = base_liquidity();
        liq.deposit(1000).unwrap();
        liq.borrow(800).unwrap();
        liq.repay(800).unwrap();
        assert_eq!(liq.available_amount, 1000);
        assert_eq!(liq.borrowed_amount_sf, 0);
    }

    #[test]
    fn test_withdraw_decreases_available() {
        let mut liq = base_liquidity();
        liq.deposit(1000).unwrap();
        liq.withdraw(400).unwrap();
        assert_eq!(liq.available_amount, 600);
    }

    #[test]
    fn test_withdraw_fails_when_insufficient() {
        let mut liq = base_liquidity();
        liq.deposit(100).unwrap();
        assert!(liq.withdraw(200).is_err());
    }

    #[test]
    fn test_total_supply_is_available_plus_borrowed() {
        let mut liq = base_liquidity();
        liq.deposit(1000).unwrap();
        liq.borrow(600).unwrap();
        // available=400, borrowed=600, total=1000
        assert_eq!(liq.total_supply().unwrap(), 1000);
    }

    #[test]
    fn test_utilization_rate_zero_when_nothing_borrowed() {
        let mut liq = base_liquidity();
        liq.deposit(1000).unwrap();
        assert_eq!(liq.utilization_rate().unwrap(), 0);
    }

    #[test]
    fn test_utilization_rate_80_percent() {
        let mut liq = base_liquidity();
        liq.deposit(1000).unwrap();
        liq.borrow(800).unwrap();
        // borrowed=800, total=1000 → 80%
        assert_eq!(liq.utilization_rate().unwrap(), 80);
    }

    #[test]
    fn test_utilization_rate_zero_when_empty_pool() {
        let liq = base_liquidity();
        assert_eq!(liq.utilization_rate().unwrap(), 0);
    }

    #[test]
    fn test_accrue_interest_fails_on_zero_slots() {
        let mut liq = base_liquidity();
        assert!(liq.accured_interest(1000, 0).is_err());
    }

    #[test]
    fn test_accrue_interest_fails_on_zero_rate() {
        let mut liq = base_liquidity();
        assert!(liq.accured_interest(0, 100).is_err());
    }

    #[test]
    fn test_accrue_interest_grows_debt() {
        let mut liq = base_liquidity();
        liq.deposit(1_000_000).unwrap();
        liq.borrow(1_000_000).unwrap();

        let debt_before = liq.borrowed_amount_sf;
        // accrue 1 full year at 10% APR
        liq.accured_interest(1000, SLOTS_PER_YEAR).unwrap();

        assert!(liq.borrowed_amount_sf > debt_before);
    }

    #[test]
    fn test_accrue_interest_grows_cumulative_rate() {
        let mut liq = base_liquidity();
        liq.deposit(1_000_000).unwrap();
        liq.borrow(500_000).unwrap();

        let rate_before = liq.cumulative_borrow_rate_sf;
        liq.accured_interest(500, SLOTS_PER_YEAR).unwrap();

        assert!(liq.cumulative_borrow_rate_sf > rate_before);
    }

    // --- Reserve ---

    fn base_reserve() -> Reserve {
        let mut reserve = Reserve::default();
        reserve.last_update_slot = 100;

        reserve.liquidity.init(NewReserveLiquidityParams {
            mint: Pubkey::default(),
            supply_vault: Pubkey::default(),
            fee_vault: Pubkey::default(),
        });

        reserve.collateral.init(NewReserveCollateralParams {
            mint_pda: Pubkey::default(),
            supply_vault_pda: Pubkey::default(),
        });

        reserve
            .config
            .init(InitReserveConfigParams {
                status: 0,
                min_borrow_rate_bps: 200,
                optimal_borrow_rate_bps: 2000,
                max_borrow_rate_bps: 10000,
                optimal_utilization_bps: 8000,
                loan_to_value_pct: 75,
                liquidation_threshold_pct: 80,
                liquidation_bonus_pct: 500,
                deposit_limit: 1_000_000,
                borrow_limit: 800_000,
                protocol_fee: 500,
            })
            .unwrap();

        reserve
    }

    #[test]
    fn test_reserve_deposit_liquidity_mints_collateral() {
        let mut reserve = base_reserve();
        let collateral_minted = reserve.deposit_liquidity(1000).unwrap();
        // first deposit at 1:1 rate → collateral == liquidity
        assert_eq!(collateral_minted, 1000);
        assert_eq!(reserve.liquidity.available_amount, 1000);
        assert_eq!(reserve.collateral.mint_total_supply, 1000);
    }

    #[test]
    fn test_reserve_deposit_fails_when_limit_crossed() {
        let mut reserve = base_reserve();
        // deposit_limit = 1_000_000, try depositing more
        assert!(reserve.deposit_liquidity(1_000_001).is_err());
    }

    #[test]
    fn test_reserve_redeem_collateral_returns_liquidity() {
        let mut reserve = base_reserve();
        reserve.deposit_liquidity(1000).unwrap();
        let liquidity_returned = reserve.redeem_collateral(500).unwrap();
        // 1:1 rate → get back 500 liquidity
        assert_eq!(liquidity_returned, 500);
        assert_eq!(reserve.collateral.mint_total_supply, 500);
        assert_eq!(reserve.liquidity.available_amount, 500);
    }

    #[test]
    fn test_reserve_borrow_moves_liquidity() {
        let mut reserve = base_reserve();
        reserve.deposit_liquidity(1000).unwrap();
        reserve.borrow(600).unwrap();
        assert_eq!(reserve.liquidity.available_amount, 400);
        assert_eq!(reserve.liquidity.borrowed_amount_sf, 600);
    }

    #[test]
    fn test_reserve_borrow_fails_when_limit_crossed() {
        let mut reserve = base_reserve();
        reserve.deposit_liquidity(1_000_000).unwrap();
        // borrow_limit = 800_000, borrow once to cross it
        reserve.borrow(800_000).unwrap();
        // now limit is crossed — next borrow should fail
        assert!(reserve.borrow(1).is_err());
    }

    #[test]
    fn test_reserve_repay_restores_liquidity() {
        let mut reserve = base_reserve();
        reserve.deposit_liquidity(1000).unwrap();
        reserve.borrow(600).unwrap();
        reserve.repay(600).unwrap();
        assert_eq!(reserve.liquidity.borrowed_amount_sf, 0);
        assert_eq!(reserve.liquidity.available_amount, 1000);
    }

    #[test]
    fn test_reserve_accrue_interest_updates_slot() {
        let mut reserve = base_reserve();
        reserve.deposit_liquidity(1_000_000).unwrap();
        reserve.borrow(800_000).unwrap();
        reserve.accrue_interest(100 + SLOTS_PER_YEAR).unwrap();
        assert_eq!(reserve.last_update_slot, 100 + SLOTS_PER_YEAR);
    }

    #[test]
    fn test_reserve_accrue_interest_skips_when_no_slots_elapsed() {
        let mut reserve = base_reserve();
        reserve.deposit_liquidity(1_000_000).unwrap();
        reserve.borrow(500_000).unwrap();
        let debt_before = reserve.liquidity.borrowed_amount_sf;
        // same slot — nothing should change
        reserve.accrue_interest(100).unwrap();
        assert_eq!(reserve.liquidity.borrowed_amount_sf, debt_before);
    }

    #[test]
    fn test_reserve_accrue_interest_grows_debt() {
        let mut reserve = base_reserve();
        reserve.deposit_liquidity(1_000_000).unwrap();
        reserve.borrow(800_000).unwrap();
        let debt_before = reserve.liquidity.borrowed_amount_sf;
        reserve.accrue_interest(100 + SLOTS_PER_YEAR).unwrap();
        assert!(reserve.liquidity.borrowed_amount_sf > debt_before);
    }

    #[test]
    fn test_reserve_deposit_limit_crossed() {
        let mut reserve = base_reserve();
        reserve.deposit_liquidity(1_000_000).unwrap();
        assert!(reserve.deposit_limit_crossed());
    }

    #[test]
    fn test_reserve_borrow_limit_crossed() {
        let mut reserve = base_reserve();
        reserve.deposit_liquidity(1_000_000).unwrap();
        reserve.borrow(800_000).unwrap();
        assert!(reserve.borrow_limit_crossed());
    }

    #[test]
    fn test_reserve_utilization_rate_after_borrow() {
        let mut reserve = base_reserve();
        reserve.deposit_liquidity(1000).unwrap();
        reserve.borrow(800).unwrap();
        assert_eq!(reserve.utilization_rate().unwrap(), 80);
    }
}
