# VeilVault

A confidential lending protocol on Solana. Users deposit collateral, borrow against it, and repay — with positions kept private via Arcium MPC. Liquidations execute correctly over encrypted data without exposing individual balances.

**Track:** DeFi + Privacy (Colosseum Frontier)

---

## What Makes It Different

Standard lending protocols (Kamino, Marginfi) store all positions in plaintext on-chain. VeilVault encrypts collateral amounts, borrow amounts, and health factors using Arcium's MPC network. The protocol enforces solvency and triggers liquidations without ever revealing a user's position to external observers.

This is the only Solana lending protocol where the liquidator learns only *whether* a position is liquidatable — not the exact collateral or debt amounts.

---

## Architecture

### Accounts

| Account | Description |
|---|---|
| `LendingMarket` | Global config — owner, quote currency, emergency pause, protocol fee |
| `Reserve` | Per-asset state — liquidity vault, collateral mint, interest model, Pyth oracle |
| `Obligation` | Per-user state — deposited collateral and borrowed amounts across reserves |
| `PrivateObligation` | Per-user encrypted state — ciphertext via Arcium MXE |

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

When a user deposits, the protocol mints cTokens at the current exchange rate. The exchange rate appreciates over time as interest accrues — cTokens automatically compound interest without per-user bookkeeping.

```
exchange_rate = total_liquidity_supply / ctoken_supply
total_liquidity_supply = available + borrowed − accumulated_protocol_fees
```

Depositing into an empty pool always mints 1:1. After interest accrues, each cToken redeems for more underlying than was deposited.

---

## Instructions

### Core Lending (15 instructions)

```
initialize_market        — create LendingMarket PDA (admin only)
add_reserve              — register a new asset; create vault, fee vault, cToken mint/supply PDAs
init_obligation          — create a per-user Obligation PDA (one per market)
deposit                  — underlying → liquidity_vault, mint cTokens to user
deposit_collateral       — lock cTokens into Obligation.deposits[] to post collateral
borrow                   — liquidity_vault → user, record debt in Obligation (requires fresh refresh)
repay                    — user → liquidity_vault, reduce Obligation debt
withdraw_collateral      — unlock cTokens from Obligation; requires refresh_obligation in same slot
withdraw                 — burn cTokens, return underlying from liquidity_vault
refresh_reserve          — accrue interest + update Pyth oracle price
refresh_obligation       — recompute borrow/collateral USD values via reserve prices
liquidate                — repay debt, seize cToken collateral with bonus (50% close factor)
set_pause                — toggle emergency pause; blocks deposit/borrow/withdraw when active
update_reserve_config    — owner-only reconfiguration of all ReserveConfig parameters
update_market_authority  — transfer market ownership to a new authority
```

### Arcium MPC Privacy Layer (11 instructions)

```
init_private_obligation     — create PrivateObligation PDA; queue init_position MPC computation
init_position_callback      — MXE writes initial encrypted PrivatePosition into enc_state
private_deposit_collateral  — SPL transfer + queue add_collateral MPC update
add_collateral_callback     — MXE updates encrypted collateral field
private_borrow              — SPL transfer + queue add_borrow MPC update (inline plaintext HF check)
add_borrow_callback         — MXE updates encrypted borrow field
private_check_liquidatable  — queue check_health MPC (computes HF over encrypted data)
check_health_callback       — set is_liquidatable flag + emit LiquidatableEvent
execute_private_liquidation — seize collateral for positions flagged is_liquidatable
init_position_comp_def      — one-time circuit registration
add_collateral_comp_def / add_borrow_comp_def / check_health_comp_def — circuit registration
```

Circuit definitions in `encrypted-ixs/`: `init_position`, `add/remove_collateral`, `add/remove_borrow`, `check_health` — all compiled via the `tools/circuit-builder` binary.

---

## Interest Rate Model

Utilization-based kinked curve — low rates at low utilization, steep rates above the optimal kink:

```
utilization = borrowed / total_supply

if utilization <= optimal:
    rate = min_rate + (utilization / optimal) × (optimal_rate − min_rate)
else:
    rate = optimal_rate + ((utilization − optimal) / (1 − optimal)) × (max_rate − optimal_rate)
```

Interest is tracked via a cumulative borrow rate index on the `Reserve`. Each `Obligation` stores the index at its last interaction. Accrual is a single multiply-then-divide — no per-user loops.

---

## Health Factor

```
HF = Σ(collateral_value × liquidation_threshold) / Σ(borrow_value)
```

- `HF ≥ 1.0` — healthy
- `HF < 1.0` — liquidatable

Oracle prices from Pyth with dual staleness checks (slot-based and timestamp-based). `borrow` requires `refresh_obligation` in the same slot to guarantee fresh prices.

---

## Privacy Layer

Arcium MPC encrypts per-user:
- Deposited collateral amounts
- Borrowed amounts
- Health factor computation inputs

Liquidation checks run over encrypted data via Arcium's MXE. The liquidator learns only whether a position is liquidatable — not the exact amounts. Six Arcis circuits handle the confidential arithmetic.

---

## Tech Stack

| Layer | Choice |
|---|---|
| Program | Rust — Anchor 0.32.1 |
| Token standard | SPL Token / Token-2022 (via `token_interface`) |
| Oracle | Pyth Network (pyth-solana-receiver-sdk 0.3.1) |
| Privacy | Arcium Arcis + MXE |
| Tests | 66 unit tests + 36 LiteSVM integration tests + 10 TypeScript smoke tests |

---

## Repository Layout

```
programs/veilvault/src/
├── lib.rs                     — program entry, instruction dispatch
├── error.rs                   — LendingError variants
├── constants.rs               — RATE_SCALE, fee/slot/count limits
├── utils/
│   └── last_update.rs         — slot + timestamp staleness checks
├── state/
│   ├── lending_market.rs      — LendingMarket account
│   ├── reserve.rs             — Reserve, ReserveConfig, ReserveLiquidity, ReserveCollateral
│   └── obligation.rs          — Obligation, ObligationCollateral, ObligationLiquidity
└── instructions/
    ├── initialize_market.rs
    ├── add_reserve.rs
    ├── init_obligation.rs
    ├── deposit.rs
    ├── deposit_collateral.rs
    ├── borrow.rs
    ├── repay.rs
    ├── withdraw_collateral.rs
    ├── withdraw.rs
    ├── refresh_reserve.rs
    ├── refresh_obligation.rs
    ├── liquidate.rs
    ├── set_pause.rs
    ├── update_reserve_config.rs
    └── update_market_authority.rs

encrypted-ixs/                — Arcium Arcis circuit definitions (6 circuits)
tools/circuit-builder/        — offline .arcis.ir → .arcis compiler
libs/veilvault-tests/         — LiteSVM Rust integration test suite (36 tests)
```

---

## Building and Testing

**Prerequisites:** Rust, Solana CLI, Anchor 0.32.1, Node.js 18+

```bash
cd veilvault

# build the BPF program
anchor build

# TypeScript smoke tests (no localnet needed — uses bankrun)
anchor test

# Rust unit tests (66 tests, no validator needed)
cargo test -p veilvault

# LiteSVM integration tests (36 tests — full deposit/borrow/repay/liquidate flows)
# First build the BPF:
cargo build-sbf
# Then run integration suite:
cargo test -p veilvault-tests
```

The LiteSVM suite covers:
- Market and reserve initialization
- Full deposit/withdraw round trips with exchange rate verification
- Borrow/repay with interest accrual
- Two-phase collateral (deposit_collateral / withdraw_collateral)
- Governance: pause, update_config, update_authority
- Liquidation: setup, trigger, bonus seizure, close factor

---

## Key Design Decisions

- **`#[account(zero_copy)]` + `#[repr(C)]`** on `Reserve` and `Obligation` — `AccountLoader` keeps them off the stack; required to stay within Solana's 4096-byte BPF frame limit
- **`Box<Account<...>>` / `Box<InterfaceAccount<...>>`** in Accounts structs — prevents stack overflow during `try_accounts` deserialization
- **`RATE_SCALE = 1_000_000_000_000` (1e12)** fixed-point for interest math — `_sf` suffix marks scaled fields; obligation borrows track debt × RATE_SCALE
- **Sub-unit dust clearing** — after repay, if remaining debt < `RATE_SCALE` (< 1 atomic token unit), the borrow slot is closed automatically
- **cToken exchange rate** — first deposit always 1:1; appreciates as interest accrues; rate = total_liquidity / ctoken_supply
- **Health factor deferred to refresh** — `borrow` requires `refresh_obligation` in the current slot; stale obligations cannot borrow
- **Simpler than Kamino by design** — no elevation groups, withdrawal tickets, farms, or referral tiers
- **Arcium dual-path** — plaintext path (standard instructions) and encrypted path (private instructions) coexist; users opt into privacy
