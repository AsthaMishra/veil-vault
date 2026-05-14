use arcis::*;

// VeilVault confidential position circuit.
//
// PrivatePosition tracks a user's collateral and borrow amounts in whole-token units.
// All amounts are stored in the MXE as Enc<Mxe, PrivatePosition> ciphertexts.
// Health checks run entirely inside the MXE — only a bool result is ever revealed.
//
// Arithmetic precision note:
//   Amounts are in whole-token units (atomic / 10^decimals), capped at ~10^9 tokens.
//   Prices are in USD cents per whole token (e.g., $100 SOL → 10_000 cents).
//   exchange_rate_bps is the cToken→underlying rate × 10_000 (e.g., 1.02 → 10_200).
//   Intermediate products stay within u64 range:
//     step1: tokens × exchange_rate_bps × ltv_bps ≤ 10^9 × 2×10^4 × 10^4 = 2×10^17 < 2^64 ✓
//     step2: / 10^8 ≤ 2×10^9
//     step3: × price_cents ≤ 2×10^9 × 10^6 = 2×10^15 < 2^64 ✓

#[encrypted]
mod circuits {
    use arcis::*;

    /// Encrypted user position: collateral locked + amount borrowed (both in whole tokens).
    pub struct PrivatePosition {
        /// Whole cToken count locked as collateral (deposited_amount / 10^decimals).
        pub collateral_tokens: u64,
        /// Whole underlying token count borrowed (borrowed_amount / 10^decimals).
        pub borrow_tokens: u64,
    }

    /// Initializes an empty PrivatePosition encrypted for the MXE.
    /// Called once when creating a PrivateObligation.
    #[instruction]
    pub fn init_position_v2() -> Enc<Mxe, PrivatePosition> {
        let pos = PrivatePosition {
            collateral_tokens: 0,
            borrow_tokens: 0,
        };
        Mxe::get().from_arcis(pos)
    }

    /// Increases the encrypted collateral amount by `amount` whole cTokens.
    /// Called after a deposit_collateral token transfer is confirmed on-chain.
    #[instruction]
    pub fn add_collateral_v2(
        amount: Enc<Shared, u64>,
        state: Enc<Mxe, PrivatePosition>,
    ) -> Enc<Mxe, PrivatePosition> {
        let delta = amount.to_arcis();
        let mut pos = state.to_arcis();
        pos.collateral_tokens += delta;
        state.owner.from_arcis(pos)
    }

    /// Decreases the encrypted collateral amount by `amount` whole cTokens.
    /// Saturates to 0 rather than underflowing — on-chain token checks prevent invalid withdrawals.
    #[instruction]
    pub fn remove_collateral_2(
        amount: Enc<Shared, u64>,
        state: Enc<Mxe, PrivatePosition>,
    ) -> Enc<Mxe, PrivatePosition> {
        let delta = amount.to_arcis();
        let mut pos = state.to_arcis();
        // Both branches always execute (Arcis circuit constraint).
        let decreased = if pos.collateral_tokens >= delta {
            pos.collateral_tokens - delta
        } else {
            0u64
        };
        pos.collateral_tokens = decreased;
        state.owner.from_arcis(pos)
    }

    /// Increases the encrypted borrow amount by `amount` whole underlying tokens.
    /// Called after a borrow token transfer is confirmed on-chain.
    #[instruction]
    pub fn add_borrow_v2(
        amount: Enc<Shared, u64>,
        state: Enc<Mxe, PrivatePosition>,
    ) -> Enc<Mxe, PrivatePosition> {
        let delta = amount.to_arcis();
        let mut pos = state.to_arcis();
        pos.borrow_tokens += delta;
        state.owner.from_arcis(pos)
    }

    /// Decreases the encrypted borrow amount by `amount` whole underlying tokens.
    /// Saturates to 0 — on-chain checks ensure repay amounts are valid.
    #[instruction]
    pub fn remove_borrow_2(
        amount: Enc<Shared, u64>,
        state: Enc<Mxe, PrivatePosition>,
    ) -> Enc<Mxe, PrivatePosition> {
        let delta = amount.to_arcis();
        let mut pos = state.to_arcis();
        let decreased = if pos.borrow_tokens >= delta {
            pos.borrow_tokens - delta
        } else {
            0u64
        };
        pos.borrow_tokens = decreased;
        state.owner.from_arcis(pos)
    }

    /// Returns true iff the position's collateral value covers its borrow value.
    ///
    /// Public inputs (derived from on-chain Pyth prices + reserve config, never private):
    ///   exchange_rate_bps — cToken-to-underlying rate × 10_000 (e.g., 1.02 → 10_200)
    ///   collateral_price_cents — USD price of 1 whole underlying collateral token in cents
    ///   borrow_price_cents    — USD price of 1 whole underlying borrow token in cents
    ///   ltv_bps               — liquidation threshold × 10_000 (e.g., 85% → 8_500)
    ///
    /// Formula (avoids u64 overflow by dividing before each multiply):
    ///   coll_adj   = collateral_tokens × exchange_rate_bps × ltv_bps / 100_000_000
    ///   coll_value = coll_adj × collateral_price_cents
    ///   borr_value = borrow_tokens × borrow_price_cents
    ///   is_healthy = coll_value >= borr_value
    #[instruction]
    pub fn check_health_v2(
        state: Enc<Mxe, PrivatePosition>,
        exchange_rate_bps: u64,
        collateral_price_cents: u64,
        borrow_price_cents: u64,
        ltv_bps: u64,
    ) -> bool {
        let pos = state.to_arcis();

        // Divide by 10^8 (= 10_000 × 10_000) after the two BPS multiplications to stay in u64.
        let coll_adj = pos.collateral_tokens * exchange_rate_bps * ltv_bps / 100_000_000;
        let coll_value = coll_adj * collateral_price_cents;
        let borr_value = pos.borrow_tokens * borrow_price_cents;

        (coll_value >= borr_value).reveal()
    }
}
