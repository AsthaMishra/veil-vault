use anchor_lang::prelude::*;
use pyth_solana_receiver_sdk::price_update::{PriceFeedMessage, PriceUpdateV2, VerificationLevel};

use crate::{constants::RATE_SCALE, error::LendingError};

/// Max allowed conf/price ratio: 2% — same threshold as Kamino.
const CONFIDENCE_FACTOR: u64 = 50; // 100 / 2

/// Reads a Pyth PriceUpdateV2 account, validates full verification level,
/// staleness, and confidence interval, then returns the USD price scaled
/// by RATE_SCALE (so $1.00 = RATE_SCALE = 1_000_000_000_000).
pub fn get_pyth_price(
    pyth_price_info: &AccountInfo,
    max_age_secs: i64,
    current_timestamp: i64,
) -> Result<(u128, i64)> {
    let data = pyth_price_info.data.borrow();
    let price_update =
        PriceUpdateV2::try_deserialize(&mut data.as_ref()).map_err(|_| error!(LendingError::PriceNotValid))?;

    if !price_update.verification_level.gte(VerificationLevel::Full) {
        return err!(LendingError::PriceNotValid);
    }

    let PriceFeedMessage {
        price,
        conf,
        exponent,
        publish_time,
        ..
    } = price_update.price_message;

    // staleness check
    require!(
        current_timestamp.saturating_sub(publish_time) <= max_age_secs,
        LendingError::PriceStale
    );

    // price must be positive
    require!(price > 0, LendingError::PriceNotValid);
    let price_u64 = price as u64;

    // confidence check: conf × CONFIDENCE_FACTOR ≤ price  ↔  conf/price ≤ 1/CONFIDENCE_FACTOR = 2%
    require!(
        conf.saturating_mul(CONFIDENCE_FACTOR) <= price_u64,
        LendingError::PriceConfidenceTooWide
    );

    // Convert Pyth fixed-point to RATE_SCALE integer.
    // Pyth gives: actual_price = price × 10^exponent
    // We want:    price_sf = actual_price × RATE_SCALE  (integer)
    let exp_abs = exponent.unsigned_abs() as u32;
    let price_sf: u128 = if exponent < 0 {
        // actual_price = price / 10^exp_abs  →  price_sf = price × RATE_SCALE / 10^exp_abs
        (price_u64 as u128)
            .checked_mul(RATE_SCALE)
            .and_then(|p| p.checked_div(10u128.pow(exp_abs)))
            .ok_or(LendingError::MathOverflow)?
    } else {
        // actual_price = price × 10^exp  →  price_sf = price × RATE_SCALE × 10^exp
        (price_u64 as u128)
            .checked_mul(RATE_SCALE)
            .and_then(|p| p.checked_mul(10u128.pow(exp_abs)))
            .ok_or(LendingError::MathOverflow)?
    };

    Ok((price_sf, publish_time))
}
