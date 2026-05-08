use anchor_lang::prelude::*;

use crate::{
    constants::RATE_SCALE,
    error::LendingError,
    state::{LendingMarket, Obligation, Reserve},
};

#[derive(Accounts)]
pub struct RefreshObligation<'info> {
    pub lending_market: Box<Account<'info, LendingMarket>>,

    #[account(
        mut,
        has_one = lending_market,
    )]
    pub obligation: AccountLoader<'info, Obligation>,
    // remaining_accounts: all Reserve accounts referenced by this obligation's
    // borrows[] and deposits[] slots (in any order).
}

/// Reads fields from a Reserve AccountInfo by borrowing the raw data in a
/// scoped block. Returns owned values so no lifetime escapes.
/// AccountLoader internally does the same bytemuck cast after skipping the
/// 8-byte Anchor discriminator.
fn read_reserve_fields(account: &AccountInfo) -> Result<(u128, u128, u128, u8, u128)> {
    let data = account.try_borrow_data()?;
    let size = std::mem::size_of::<Reserve>();
    if data.len() < 8 + size {
        return err!(LendingError::InvalidConfig);
    }
    let reserve = bytemuck::try_from_bytes::<Reserve>(&data[8..8 + size])
        .map_err(|_| error!(LendingError::InvalidConfig))?;
    Ok((
        reserve.liquidity.market_price_sf,
        reserve.liquidity.total_supply()?,
        reserve.collateral.mint_total_supply as u128,
        reserve.config.liquidation_threshold_pct,
        reserve.liquidity.cumulative_borrow_rate_sf,
    ))
}

pub fn refresh_obligation(ctx: Context<RefreshObligation>) -> Result<()> {
    let clock = Clock::get()?;
    let mut obligation = ctx.accounts.obligation.load_mut()?;

    // ── price each active borrow slot ──────────────────────────────────────
    for i in 0..obligation.borrows_count as usize {
        if !obligation.borrows[i].is_active() {
            continue;
        }
        let reserve_key = obligation.borrows[i].borrow_reserve;

        let reserve_account = ctx
            .remaining_accounts
            .iter()
            .find(|a| a.key() == reserve_key)
            .ok_or_else(|| {
                msg!("Reserve {} not found in remaining_accounts", reserve_key);
                error!(LendingError::BorrowNotFound)
            })?;

        let (price_sf, _, _, _, cumulative_rate_sf) = read_reserve_fields(reserve_account)?;
        if price_sf == 0 {
            // oracle not yet refreshed via refresh_reserve — leave market_value_sf at 0
            continue;
        }

        // accrue interest to current reserve rate so market_value_sf reflects real debt
        obligation.accrue_interest(i, cumulative_rate_sf)?;

        // borrow value in USD × RATE_SCALE
        // borrowed_amount_sf = raw_amount × RATE_SCALE  →  /RATE_SCALE gives raw_amount
        // market_value_sf    = raw_amount × price_sf    (USD × RATE_SCALE)
        let raw_amount = obligation.borrows[i].borrowed_amount_sf / RATE_SCALE;
        obligation.borrows[i].market_value_sf = raw_amount
            .checked_mul(price_sf)
            .ok_or(LendingError::MathOverflow)?;
    }

    // ── price each active deposit slot ─────────────────────────────────────
    for i in 0..obligation.deposits_count as usize {
        if !obligation.deposits[i].is_active() {
            continue;
        }
        let reserve_key = obligation.deposits[i].deposit_reserve;

        let reserve_account = ctx
            .remaining_accounts
            .iter()
            .find(|a| a.key() == reserve_key)
            .ok_or_else(|| {
                msg!("Reserve {} not found in remaining_accounts", reserve_key);
                error!(LendingError::DepositNotFound)
            })?;

        let (price_sf, total_liquidity, ctoken_supply, liq_threshold_pct, _) =
            read_reserve_fields(reserve_account)?;
        if price_sf == 0 {
            continue;
        }

        // cToken → underlying via exchange rate, then adjust for liquidation threshold
        let ctoken_amount = obligation.deposits[i].deposited_amount as u128;
        let underlying = if ctoken_supply == 0 {
            0u128
        } else {
            ctoken_amount
                .checked_mul(total_liquidity)
                .and_then(|v| v.checked_div(ctoken_supply))
                .ok_or(LendingError::MathOverflow)?
        };

        obligation.deposits[i].market_value_sf = underlying
            .checked_mul(price_sf)
            .and_then(|v| v.checked_mul(liq_threshold_pct as u128))
            .and_then(|v| v.checked_div(100))
            .ok_or(LendingError::MathOverflow)?;
    }

    // stamp the obligation with the current slot so borrow can check staleness
    obligation
        .last_update
        .update(clock.slot, clock.unix_timestamp);

    Ok(())
}
