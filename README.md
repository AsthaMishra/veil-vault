# VeilVault

A confidential lending protocol on Solana. Users deposit collateral, borrow against it, and repay — with positions kept private via Arcium MPC. Liquidations execute correctly over encrypted data without exposing individual balances.

---

## What Makes It Different

Standard lending protocols (Kamino, Marginfi) store all positions in plaintext on-chain. VeilVault encrypts collateral amounts, borrow amounts, and health factors using Arcium's MPC network. The protocol enforces solvency and triggers liquidations without ever revealing a user's position to external observers.

---

## Architecture

### Accounts

| Account | Description |
|---|---|
| `LendingMarket` | Global config — owner, quote currency, emergency pause, protocol fee |
| `Reserve` | Per-asset state — liquidity vault, collateral mint, interest model, oracle config |
| `Obligation` | Per-user state — deposited collateral and borrowed amounts across reserves |

### PDAs

```
LendingMarket:   ["lending_market", owner]
Reserve:         ["reserve", lending_market, mint]
Liquidity vault: ["liquidity_vault", reserve]
Fee vault:       ["fee_vault", reserve]
Collateral mint: ["collateral_mint", reserve]
Obligation:      ["obligation", lending_market, user]
```

### Collateral Tokens (cTokens)

When a user deposits, the protocol mints cTokens at the current exchange rate. The exchange rate appreciates over time as interest accrues — meaning cTokens automatically compound interest without any per-user bookkeeping.

```
exchange_rate = total_liquidity_supply / ctoken_supply
```

`total_liquidity_supply = available + borrowed − accumulated_protocol_fees`

---

## Interest Rate Model

Utilization-based kinked curve — low rates at low utilization, steep rates above the optimal kink to incentivize repayment.

```
utilization = borrowed / total_supply

if utilization <= optimal:
    rate = min_rate + (utilization / optimal) × (optimal_rate − min_rate)
else:
    rate = optimal_rate + ((utilization − optimal) / (1 − optimal)) × (max_rate − optimal_rate)
```

Interest is tracked via a cumulative borrow rate index stored on the `Reserve`. Each user's `Obligation` stores the index at the time of their last interaction. Accrual is a single multiply-then-divide — no per-user loops.

---

## Health Factor

```
HF = Σ(collateral_value × liquidation_threshold) / Σ(borrow_value)
```

- `HF ≥ 1.0` — healthy
- `HF < 1.0` — liquidatable

Oracle prices come from Pyth with staleness checks (slot-based and timestamp-based).

---

## Privacy Layer

Arcium MPC integration encrypts:
- Deposited collateral amounts per user
- Borrowed amounts per user  
- Health factor computation inputs

Liquidation checks run over encrypted data via Arcium's MXE — the liquidator learns only whether a position is liquidatable, not the exact amounts.

---

## Tech Stack

| Layer | Choice |
|---|---|
| Program | Rust — Anchor 0.32.1 |
| Token standard | SPL Token (Token-2022 path open for confidential transfers) |
| Oracle | Pyth Network |
| Privacy | Arcium Arcis + MXE |
| Tests | Rust unit tests + TypeScript (Anchor test framework) |
| Frontend | Next.js 15 + Tailwind + Solana wallet adapter |

---

## Program Structure

```
programs/veilvault/src/
├── lib.rs                  — program entry, instruction dispatch
├── error.rs                — LendingError variants
├── constants.rs            — RATE_SCALE, fee/slot/count limits
├── utils/
│   └── last_update.rs      — slot + timestamp staleness checks
├── state/
│   ├── lending_market.rs   — LendingMarket account
│   ├── reserve.rs          — Reserve, ReserveConfig, ReserveLiquidity, ReserveCollateral
│   └── obligation.rs       — Obligation, ObligationCollateral, ObligationLiquidity
└── instructions/
    ├── initialize_market.rs
    ├── add_reserve.rs
    ├── deposit.rs          (planned)
    ├── borrow.rs           (planned)
    ├── repay.rs            (planned)
    ├── withdraw.rs         (planned)
    ├── refresh_reserve.rs  (planned)
    ├── refresh_obligation.rs (planned)
    └── liquidate.rs        (planned)
```

---

## Building and Testing

**Prerequisites**: Solana CLI, Anchor 0.32.1, Rust, Node.js

```bash
cd veilvault

# build the program
anchor build

# run all tests against localnet
anchor test
```

Unit tests (Rust) live inside each `state/` module and can be run independently:

```bash
cargo test -p veilvault
```

---

## Key Design Decisions

- `zero_copy` + `#[repr(C)]` for large accounts (`Reserve`, `Obligation`) — avoids Solana stack overflow on deserialization
- `RATE_SCALE = 1_000_000_000_000` (1e12) fixed-point precision for interest math
- `_sf` suffix on fields that store scaled integers (e.g. `borrowed_amount_sf`)
- Checked arithmetic everywhere — no unchecked `+`/`-` on financial values
- Clean state/instructions boundary — state structs contain pure logic with no Anchor or CPI calls; instructions own all account validation and CPI
- Simpler than Kamino by design — no elevation groups, withdrawal tickets, farms, or referral tiers
