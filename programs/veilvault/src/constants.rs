pub const BPS_SCALER: u64 = 10_000;
pub const PERCENT_SCALER: u128 = 100;
pub const MAX_PROTOCOL_FEE_BPS: u16 = 1_000; // 10% max

pub const SLOTS_PER_SECOND: u64 = 2;
pub const SLOTS_PER_MINUTE: u64 = 60 * SLOTS_PER_SECOND;
pub const SLOTS_PER_HOUR: u64 = 60 * SLOTS_PER_MINUTE;
pub const SLOTS_PER_DAY: u64 = 24 * SLOTS_PER_HOUR;
pub const SLOTS_PER_YEAR: u64 = 365 * SLOTS_PER_DAY;

pub const RATE_SCALE: u128 = 1_000_000_000_000; // 1e12 precision

pub const MAX_LIQUIDATION_THRESHOLD_PCT: u8 = 100;
pub const MAX_UTILIZATION_BPS: u16 = 10_000;

pub const MAX_AGE_SECONDS: i64 = 60; // price stale after 60s
pub const MAX_DEPOSITS_COUNT_IN_RESERVE: usize = 8;
pub const MAX_BORROW_COUNT_IN_RESERVE: usize = 8;

/// Max fraction of an unhealthy obligation's debt repayable in one liquidation (50%).
pub const MAX_LIQUIDATION_CLOSE_FACTOR_PCT: u128 = 50;
