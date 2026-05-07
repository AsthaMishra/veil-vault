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
LendingMarket:      ["lending_market", owner]
Reserve:            ["reserve", lending_market, mint]
Liquidity vault:    ["liquidity_vault", reserve]
Fee vault:          ["fee_vault", reserve]
Collateral mint:    ["collateral_mint", reserve]
Collateral supply:  ["collateral_supply", reserve]
Obligation:         ["obligation", lending_market, user]
```

### Collateral Tokens (cTokens)

When a user deposits, the protocol mints cTokens at the current exchange rate. The exchange rate appreciates over time as interest accrues — meaning cTokens automatically compound interest without any per-user bookkeeping.

```
exchange_rate = total_liquidity_supply / ctoken_supply
```

`total_liquidity_supply = available + borrowed − accumulated_protocol_fees`

Depositing into an empty pool always mints 1:1. After interest accrues, each cToken redeems for more than 1 underlying token.

---

## Instruction Flow

```
initialize_market   — create LendingMarket PDA (admin only)
add_reserve         — register a new asset with config + mint all vault/cToken PDAs
init_obligation     — create a per-user Obligation PDA (one per market)
deposit             — transfer underlying → liquidity_vault, mint cTokens to user
borrow              — transfer underlying from vault → user, record debt in Obligation
repay               — transfer underlying from user → vault, reduce Obligation debt
withdraw            (planned) — burn cTokens, redeem underlying
refresh_reserve     (planned) — accrue interest + update oracle price
refresh_obligation  (planned) — recompute borrow values via cumulative rate ratio
liquidate           (planned) — seize collateral from unhealthy positions
```

Interest is accrued inline on every `deposit`, `borrow`, and `repay` call — no separate refresh required for basic flows.

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

Oracle prices come from Pyth with staleness checks (slot-based and timestamp-based). Health factor enforcement is wired into `borrow` once oracle integration is complete (Day 3).

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
| Token standard | SPL Token / Token-2022 (via `token_interface`) |
| Oracle | Pyth Network (planned Day 3) |
| Privacy | Arcium Arcis + MXE (planned Day 5) |
| Tests | Rust unit tests + TypeScript integration (Anchor) |
| Frontend | Next.js 15 + Tailwind + Solana wallet adapter (planned Day 8) |

---

## Program Structure

```
programs/veilvault/src/
├── lib.rs                    — program entry, instruction dispatch
├── error.rs                  — LendingError variants
├── constants.rs              — RATE_SCALE, fee/slot/count limits
├── utils/
│   └── last_update.rs        — slot + timestamp staleness checks
├── state/
│   ├── lending_market.rs     — LendingMarket account
│   ├── reserve.rs            — Reserve, ReserveConfig, ReserveLiquidity, ReserveCollateral
│   └── obligation.rs         — Obligation, ObligationCollateral, ObligationLiquidity
└── instructions/
    ├── initialize_market.rs  — ✓ done
    ├── add_reserve.rs        — ✓ done
    ├── init_obligation.rs    — ✓ done
    ├── deposit.rs            — ✓ done
    ├── borrow.rs             — ✓ done
    ├── repay.rs              — ✓ done
    ├── withdraw.rs           — planned
    ├── refresh_reserve.rs    — planned
    ├── refresh_obligation.rs — planned
    └── liquidate.rs          — planned
```

---

## Building and Testing

**Prerequisites**: Solana CLI, Anchor 0.32.1, Rust, Node.js

```bash
cd veilvault

# build the BPF program
anchor build

# run integration tests against localnet
anchor test
```

Rust unit tests live inside each `state/` module (66 tests across `reserve.rs` and `obligation.rs`) and run without a validator:

```bash
cargo test
```

---

## Key Design Decisions

- `#[account(zero_copy)]` + `#[repr(C)]` for large accounts (`Reserve`, `Obligation`) — `AccountLoader` keeps them off the stack; required to stay within Solana's 4096-byte BPF frame limit
- `Box<Account<...>>` / `Box<InterfaceAccount<...>>` for large accounts in `Accounts` structs — prevents stack overflow during `try_accounts` deserialization
- `RATE_SCALE = 1_000_000_000_000` (1e12) fixed-point precision for interest math — obligation debt tracked in scaled units, reserve debt in raw token units
- `_sf` suffix on scaled fields (e.g. `borrowed_amount_sf`, `cumulative_borrow_rate_sf`)
- Sub-unit dust cleared on repay — if remaining obligation debt < `RATE_SCALE` (less than one atomic token unit) after repayment, the borrow slot is closed automatically
- Checked arithmetic everywhere — no unchecked `+`/`-` on financial values
- Clean state/instructions split — state structs are pure Rust with no Anchor or CPI calls; instructions own all account validation and CPI
- Simpler than Kamino by design — no elevation groups, withdrawal tickets, farms, or referral tiers
