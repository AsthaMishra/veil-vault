use anchor_lang::prelude::*;

use crate::{
    constants::{
        BPS_SCALER, MAX_LIQUIDATION_THRESHOLD_PCT, MAX_PROTOCOL_FEE_BPS, MAX_UTILIZATION_BPS,
        PERCENT_SCALER, RATE_SCALE, SLOTS_PER_YEAR,
    },
    error::LendingError,
};

pub const RESERVE_VERSION: u64 = 1;

// ─── Reserve ────────────────────────────────────────────────────────────────
//
// Field order chosen so each field starts at its natural alignment boundary
// with no implicit compiler-inserted gaps (required for bytemuck::Pod).
//
// Offsets:
//   liquidity   (align 16, 160 bytes): offset    0
//   config      (align  8,  32 bytes): offset  160
//   collateral  (align  8, 1096 bytes): offset  192
//   version     (align  8,   8 bytes): offset 1288
//   last_update_slot                  offset 1296
//   lending_market (align 1, 32 bytes): offset 1304
//   bump        (align  1,   1 byte):  offset 1336
//   padding     (align  1,   7 bytes): offset 1337 → total 1344, 1344 % 16 = 0 ✓

#[account(zero_copy)]
#[repr(C)]
#[derive(Debug)]
pub struct Reserve {
    pub liquidity: ReserveLiquidity,
    pub config: ReserveConfig,
    pub collateral: ReserveCollateral,
    pub version: u64,
    pub last_update_slot: u64,
    pub lending_market: Pubkey,
    pub bump: u8,
    pub padding: [u8; 7],
}

impl Default for Reserve {
    fn default() -> Self {
        let mut r: Self = bytemuck::Zeroable::zeroed();
        r.liquidity.cumulative_borrow_rate_sf = RATE_SCALE;
        r
    }
}

impl Reserve {
    pub fn collateral_exchange_rate(&self) -> CollateralExchangeRate {
        self.collateral
            .exchange_rate(self.liquidity.total_supply().unwrap_or(0))
    }

    pub fn utilization_rate(&self) -> Result<u128> {
        self.liquidity.utilization_rate()
    }

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

    pub fn deposit_liquidity(&mut self, amount: u64) -> Result<u64> {
        let total = self.liquidity.total_supply()?;
        require!(
            total.saturating_add(amount as u128) <= self.config.deposit_limit as u128,
            LendingError::DepositLimitExceeded
        );

        let exchange_rate = self.collateral_exchange_rate();
        let collateral_amount = exchange_rate.liquidity_to_collateral(amount)?;

        self.liquidity.deposit(amount)?;
        self.collateral.mint(collateral_amount)?;

        Ok(collateral_amount)
    }

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
            self.liquidity.accured_interest(
                borrow_rate as u64,
                slots_elapsed,
                self.config.protocol_fee as u64,
            )?;
        }

        self.last_update_slot = current_slot;
        Ok(())
    }
}

// ─── ReserveConfig ──────────────────────────────────────────────────────────
//
// Offsets (no gaps):
//   deposit_limit (u64):             offset  0
//   borrow_limit  (u64):             offset  8
//   min_borrow_rate_bps (u16):       offset 16
//   optimal_borrow_rate_bps (u16):   offset 18
//   max_borrow_rate_bps (u16):       offset 20
//   optimal_utilization_bps (u16):   offset 22
//   liquidation_bonus_pct (u16):     offset 24
//   protocol_fee (u16):              offset 26
//   status (u8):                     offset 28
//   loan_to_value_pct (u8):          offset 29
//   liquidation_threshold_pct (u8):  offset 30
//   padding (u8):                    offset 31 → total 32, 32 % 8 = 0 ✓

#[zero_copy]
#[repr(C)]
#[derive(Debug)]
pub struct ReserveConfig {
    pub deposit_limit: u64,
    pub borrow_limit: u64,
    pub min_borrow_rate_bps: u16,
    pub optimal_borrow_rate_bps: u16,
    pub max_borrow_rate_bps: u16,
    pub optimal_utilization_bps: u16,
    pub liquidation_bonus_pct: u16,
    pub protocol_fee: u16,
    pub status: u8,
    pub loan_to_value_pct: u8,
    pub liquidation_threshold_pct: u8,
    pub padding: u8,
}

impl Default for ReserveConfig {
    fn default() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}

#[derive(AnchorSerialize, AnchorDeserialize)]
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
        self.deposit_limit = params.deposit_limit;
        self.borrow_limit = params.borrow_limit;
        self.min_borrow_rate_bps = params.min_borrow_rate_bps;
        self.optimal_borrow_rate_bps = params.optimal_borrow_rate_bps;
        self.max_borrow_rate_bps = params.max_borrow_rate_bps;
        self.optimal_utilization_bps = params.optimal_utilization_bps;
        self.liquidation_bonus_pct = params.liquidation_bonus_pct;
        self.protocol_fee = params.protocol_fee;
        self.status = params.status;
        self.loan_to_value_pct = params.loan_to_value_pct;
        self.liquidation_threshold_pct = params.liquidation_threshold_pct;
        self.padding = 0;
        self.validate()?;
        Ok(())
    }

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

    pub fn borrow_rate(&self, utilization_bps: u16) -> Result<u16> {
        let util = utilization_bps as u64;
        let optimal_util = self.optimal_utilization_bps as u64;

        let rate: u64 = if util <= optimal_util {
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

// ─── ReserveLiquidity ────────────────────────────────────────────────────────
//
// u128 fields must come first (align 16). Pubkeys (align 1) follow.
// Offsets:
//   borrowed_amount_sf (u128):           offset   0
//   cumulative_borrow_rate_sf (u128):    offset  16
//   accumulated_protocol_fees (u128):    offset  32
//   mint (Pubkey):                       offset  48
//   supply_vault (Pubkey):               offset  80
//   fee_vault (Pubkey):                  offset 112
//   available_amount (u64):              offset 144
//   padding ([u8;8]):                    offset 152 → total 160, 160 % 16 = 0 ✓

pub struct NewReserveLiquidityParams {
    pub mint: Pubkey,
    pub supply_vault: Pubkey,
    pub fee_vault: Pubkey,
}

#[zero_copy]
#[repr(C)]
#[derive(Debug)]
pub struct ReserveLiquidity {
    pub borrowed_amount_sf: u128,
    pub cumulative_borrow_rate_sf: u128,
    pub accumulated_protocol_fees: u128,
    pub mint: Pubkey,
    pub supply_vault: Pubkey,
    pub fee_vault: Pubkey,
    pub available_amount: u64,
    pub padding: [u8; 8],
}

impl Default for ReserveLiquidity {
    fn default() -> Self {
        let mut l: Self = bytemuck::Zeroable::zeroed();
        l.cumulative_borrow_rate_sf = RATE_SCALE;
        l
    }
}

impl ReserveLiquidity {
    pub fn init(&mut self, params: NewReserveLiquidityParams) {
        self.borrowed_amount_sf = 0;
        self.cumulative_borrow_rate_sf = RATE_SCALE;
        self.accumulated_protocol_fees = 0;
        self.mint = params.mint;
        self.supply_vault = params.supply_vault;
        self.fee_vault = params.fee_vault;
        self.available_amount = 0;
        self.padding = [0; 8];
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

    pub fn accured_interest(
        &mut self,
        borrow_rate: u64,
        slot_elapsed: u64,
        protocol_fee: u64,
    ) -> Result<()> {
        require!(slot_elapsed > 0, LendingError::ZeroSlotsElapsed);
        require!(borrow_rate > 0, LendingError::BorrowRateZeroFound);

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

        let fee = debt_accured
            .checked_mul(protocol_fee as u128)
            .and_then(|f| f.checked_div(BPS_SCALER as u128))
            .ok_or(LendingError::MathOverflow)?;

        self.accumulated_protocol_fees = self
            .accumulated_protocol_fees
            .checked_add(fee)
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
            .and_then(|sum| sum.checked_sub(self.accumulated_protocol_fees))
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

// ─── ReserveCollateral ───────────────────────────────────────────────────────
//
// Current field order has no implicit gaps:
//   mint_pda (Pubkey, align 1):        offset   0
//   mint_total_supply (u64, align 8):  offset  32  (32 % 8 = 0 ✓)
//   supply_vault_pda (Pubkey, align 1): offset  40
//   padding1 ([u64;64], align 8):      offset  72  (72 % 8 = 0 ✓)
//   padding2 ([u64;64], align 8):      offset 584  (584 % 8 = 0 ✓)
//   Total: 1096, 1096 % 8 = 0 ✓

pub struct NewReserveCollateralParams {
    pub mint_pda: Pubkey,
    pub supply_vault_pda: Pubkey,
}

#[zero_copy]
#[repr(C)]
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
        bytemuck::Zeroable::zeroed()
    }
}

impl ReserveCollateral {
    pub fn init(&mut self, params: NewReserveCollateralParams) {
        self.mint_pda = params.mint_pda;
        self.mint_total_supply = 0;
        self.supply_vault_pda = params.supply_vault_pda;
        // padding arrays stay zero (account is zero-initialised by Solana)
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

    pub fn exchange_rate(&self, total_liquidity: u128) -> CollateralExchangeRate {
        if self.mint_total_supply == 0 || total_liquidity == 0 {
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

// ─── CollateralExchangeRate ──────────────────────────────────────────────────

pub struct CollateralExchangeRate {
    collateral_supply: u128,
    total_liquidity: u128,
}

impl CollateralExchangeRate {
    pub fn collateral_to_liquidity(&self, collateral_amount: u64) -> Result<u64> {
        let result = (collateral_amount as u128)
            .checked_mul(self.total_liquidity)
            .and_then(|n| n.checked_div(self.collateral_supply))
            .ok_or(LendingError::MathOverflow)?;
        Ok(result as u64)
    }

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
            deposit_limit: 1_000_000,
            borrow_limit: 800_000,
            min_borrow_rate_bps: 200,
            optimal_borrow_rate_bps: 2000,
            max_borrow_rate_bps: 10000,
            optimal_utilization_bps: 8000,
            liquidation_bonus_pct: 500,
            protocol_fee: 500,
            status: 0,
            loan_to_value_pct: 75,
            liquidation_threshold_pct: 80,
            padding: 0,
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
        config.loan_to_value_pct = 80;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_borrow_rates_must_be_ordered() {
        let mut config = base_config();
        config.optimal_borrow_rate_bps = 100;
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
        let rate = base_config().borrow_rate(0).unwrap();
        assert_eq!(rate, 200);
    }

    #[test]
    fn test_borrow_rate_at_optimal_utilization() {
        let rate = base_config().borrow_rate(8000).unwrap();
        assert_eq!(rate, 2000);
    }

    #[test]
    fn test_borrow_rate_at_max_utilization() {
        let rate = base_config().borrow_rate(10000).unwrap();
        assert_eq!(rate, 10000);
    }

    #[test]
    fn test_borrow_rate_midpoint_segment1() {
        // rate = 200 + 4000 * (2000 - 200) / 8000 = 200 + 900 = 1100
        let rate = base_config().borrow_rate(4000).unwrap();
        assert_eq!(rate, 1100);
    }

    #[test]
    fn test_borrow_rate_midpoint_segment2() {
        // rate = 2000 + (9000-8000) * (10000-2000) / (10000-8000) = 2000 + 4000 = 6000
        let rate = base_config().borrow_rate(9000).unwrap();
        assert_eq!(rate, 6000);
    }

    // --- CollateralExchangeRate ---

    #[test]
    fn test_exchange_rate_one_to_one() {
        let rate = CollateralExchangeRate {
            collateral_supply: 100,
            total_liquidity: 100,
        };
        assert_eq!(rate.collateral_to_liquidity(50).unwrap(), 50);
        assert_eq!(rate.liquidity_to_collateral(50).unwrap(), 50);
    }

    #[test]
    fn test_exchange_rate_grown() {
        let rate = CollateralExchangeRate {
            collateral_supply: 100,
            total_liquidity: 108,
        };
        assert_eq!(rate.collateral_to_liquidity(50).unwrap(), 54);
    }

    #[test]
    fn test_exchange_rate_deposit_mints_fewer_ctokens() {
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
        assert_eq!(rate.collateral_to_liquidity(50).unwrap(), 50);
    }

    // --- ReserveLiquidity ---

    fn base_liquidity() -> ReserveLiquidity {
        ReserveLiquidity {
            borrowed_amount_sf: 0,
            cumulative_borrow_rate_sf: RATE_SCALE,
            accumulated_protocol_fees: 0,
            mint: Pubkey::default(),
            supply_vault: Pubkey::default(),
            fee_vault: Pubkey::default(),
            available_amount: 0,
            padding: [0; 8],
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
        assert!(liq.accured_interest(1000, 0, 500).is_err());
    }

    #[test]
    fn test_accrue_interest_fails_on_zero_rate() {
        let mut liq = base_liquidity();
        assert!(liq.accured_interest(0, 100, 500).is_err());
    }

    #[test]
    fn test_accrue_interest_grows_debt() {
        let mut liq = base_liquidity();
        liq.deposit(1_000_000).unwrap();
        liq.borrow(1_000_000).unwrap();

        let debt_before = liq.borrowed_amount_sf;
        liq.accured_interest(1000, SLOTS_PER_YEAR, 500).unwrap();

        assert!(liq.borrowed_amount_sf > debt_before);
    }

    #[test]
    fn test_accrue_interest_grows_cumulative_rate() {
        let mut liq = base_liquidity();
        liq.deposit(1_000_000).unwrap();
        liq.borrow(500_000).unwrap();

        let rate_before = liq.cumulative_borrow_rate_sf;
        liq.accured_interest(500, SLOTS_PER_YEAR, 500).unwrap();

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
        assert_eq!(collateral_minted, 1000);
        assert_eq!(reserve.liquidity.available_amount, 1000);
        assert_eq!(reserve.collateral.mint_total_supply, 1000);
    }

    #[test]
    fn test_reserve_deposit_fails_when_limit_crossed() {
        let mut reserve = base_reserve();
        assert!(reserve.deposit_liquidity(1_000_001).is_err());
    }

    #[test]
    fn test_reserve_redeem_collateral_returns_liquidity() {
        let mut reserve = base_reserve();
        reserve.deposit_liquidity(1000).unwrap();
        let liquidity_returned = reserve.redeem_collateral(500).unwrap();
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
        reserve.borrow(800_000).unwrap();
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
