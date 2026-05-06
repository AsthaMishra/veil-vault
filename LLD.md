# VeilVault — Low Level Diagram

## Struct Layout

```mermaid
classDiagram
    class LendingMarket {
        +u8 version
        +u8 bump
        +Pubkey owner
        +bool emergency_pause
        +[u8;32] quote_currency
        +u16 protocol_fee_bps
        +[u64;64] padding
        +init(params)
        +set_emergency_pause(bool)
        +set_protocol_fees(bps)
        +is_paused() bool
    }

    class Reserve {
        +u64 version
        +u64 last_update_slot
        +Pubkey lending_market
        +ReserveConfig config
        +ReserveLiquidity liquidity
        +ReserveCollateral collateral
        +init(params)
        +deposit_liquidity(amount) u64
        +redeem_collateral(ctokens) u64
        +borrow(amount)
        +repay(amount)
        +accrue_interest(slot)
        +current_borrow_rate() u16
        +utilization_rate() u128
    }

    class ReserveConfig {
        +u8 status
        +u16 min_borrow_rate_bps
        +u16 optimal_borrow_rate_bps
        +u16 max_borrow_rate_bps
        +u16 optimal_utilization_bps
        +u8 loan_to_value_pct
        +u8 liquidation_threshold_pct
        +u16 liquidation_bonus_pct
        +u64 deposit_limit
        +u64 borrow_limit
        +u16 protocol_fee
        +borrow_rate(util_bps) u16
        +validate()
        +is_active() bool
        +is_frozen() bool
    }

    class ReserveLiquidity {
        +Pubkey mint
        +Pubkey supply_vault
        +Pubkey fee_vault
        +u64 available_amount
        +u128 borrowed_amount_sf
        +u128 cumulative_borrow_rate_sf
        +u128 accumulated_protocol_fees
        +deposit(amount)
        +borrow(amount)
        +repay(amount)
        +withdraw(amount)
        +accrue_interest(rate, slots, fee)
        +total_supply() u128
        +utilization_rate() u128
    }

    class ReserveCollateral {
        +Pubkey mint_pda
        +u64 mint_total_supply
        +Pubkey supply_vault_pda
        +mint(amount)
        +burn(amount)
        +exchange_rate(total_liquidity) CollateralExchangeRate
    }

    class CollateralExchangeRate {
        -u128 collateral_supply
        -u128 total_liquidity
        +collateral_to_liquidity(c) u64
        +liquidity_to_collateral(l) u64
    }

    class Obligation {
        +Pubkey lending_market
        +Pubkey owner
        +LastUpdate last_update
        +[ObligationCollateral;8] deposits
        +[ObligationLiquidity;8] borrows
        +u8 deposits_count
        +u8 borrows_count
        +u8 bump
        +[u8;13] padding
        +[u64;64] padding1
        +init(params)
        +deposit(reserve, amount)
        +withdraw(reserve, amount)
        +borrow(reserve, amount, rate)
        +repay(reserve, amount)
        +accrue_interest(slot_index, rate)
        +find_or_add_deposit(reserve) usize
        +find_or_add_borrow(reserve) usize
    }

    class ObligationCollateral {
        +Pubkey deposit_reserve
        +u64 deposited_amount
        +is_active() bool
        +init(reserve)
    }

    class ObligationLiquidity {
        +Pubkey borrow_reserve
        +u128 borrowed_amount_sf
        +u128 cumulative_borrow_rate_sf
        +is_active() bool
        +init(reserve, rate)
    }

    class LastUpdate {
        +u64 slot
        +i64 timestamp
        +init()
        +update(slot, timestamp)
        +is_price_stale(now) bool
        +is_slot_stale(slot) bool
    }

    Reserve *-- ReserveConfig
    Reserve *-- ReserveLiquidity
    Reserve *-- ReserveCollateral
    ReserveCollateral ..> CollateralExchangeRate : produces
    Obligation *-- LastUpdate
    Obligation *-- ObligationCollateral
    Obligation *-- ObligationLiquidity
```

---

## Memory Layout — Obligation (1440 bytes)

```
Offset   Size   Field
──────────────────────────────────────────────────────
0        32     lending_market: Pubkey
32       32     owner: Pubkey
64       16     last_update: LastUpdate (slot:u64 + timestamp:i64)
80       320    deposits: [ObligationCollateral; 8]
                  each slot = 32 (Pubkey) + 8 (u64) = 40 bytes
400      512    borrows: [ObligationLiquidity; 8]
                  each slot = 32 (Pubkey) + 16 (u128) + 16 (u128) = 64 bytes
912      1      deposits_count: u8
913      1      borrows_count: u8
914      1      bump: u8
915      13     padding: [u8;13]   ← closes gap to 16-byte boundary
928      512    padding1: [u64;64]
──────────────────────────────────────────────────────
Total:   1440   1440 % 16 == 0 ✓ no hidden gaps
```

---

## PDA Derivation Tree

```mermaid
flowchart TD
    OWNER[Owner Pubkey]
    OWNER -->|seeds: lending_market + owner| LM["LendingMarket PDA\n[lending_market, owner]"]

    LM -->|seeds: reserve + market + mint| RES["Reserve PDA\n[reserve, market, mint]"]

    RES -->|seeds: liquidity_vault + reserve| LV["Liquidity Vault\n[liquidity_vault, reserve]"]
    RES -->|seeds: fee_vault + reserve| FV["Fee Vault\n[fee_vault, reserve]"]
    RES -->|seeds: collateral_mint + reserve| CM["Collateral Mint\n[collateral_mint, reserve]"]

    LM -->|seeds: obligation + market + user| OBL["Obligation PDA\n[obligation, market, user]"]
```

---

## Interest Rate Curve

```
Borrow Rate (bps)
│
max ──────────────────────────── ●
10000│                          /
     │                         /  segment 2
     │                        /   (steep slope)
opt ─│──────────── ●
2000 │            /●
     │           / |
     │          /  |
     │         /   |  segment 1
     │        /    |  (gentle slope)
min ─│── ●   /     |
200  │   |  /      |
     │   | /       |
     └───┼─────────┼────────── Utilization (bps)
         0       8000      10000
                 optimal
```

---

## Instruction → State Call Chain

### `add_reserve`

```mermaid
flowchart LR
    IX[add_reserve instruction]
    IX --> VC[validate config\nReserveConfig.init]
    IX --> IL[init liquidity\nReserveLiquidity.init\nmint + supply_vault + fee_vault]
    IX --> IC[init collateral\nReserveCollateral.init\nmint_pda]
    VC --> RI[Reserve.init\nslot + market + liquidity\n+ collateral + config]
    IL --> RI
    IC --> RI
```

### `deposit` (planned)

```mermaid
flowchart LR
    IX[deposit instruction]
    IX --> DL["Reserve.deposit_liquidity(amount)\n→ exchange_rate()\n→ liquidity_to_collateral()\n→ liquidity.deposit()\n→ collateral.mint()"]
    IX --> OD["Obligation.deposit(reserve, ctokens)\n→ find_or_add_deposit()\n→ deposited_amount += ctokens"]
    IX --> CPI1[SPL transfer\nuser → liquidity_vault]
    IX --> CPI2[SPL mint_to\ncollateral_mint → user]
```

### `borrow` (planned)

```mermaid
flowchart LR
    IX[borrow instruction]
    IX --> HF[check health_factor\nafter simulated borrow ≥ 1.0]
    IX --> RB["Reserve.borrow(amount)\n→ liquidity.borrow()\navailable -= amount\nborrowed_sf += amount"]
    IX --> OB["Obligation.borrow(reserve, amount, rate)\n→ find_or_add_borrow()\n→ borrowed_amount_sf += amount × RATE_SCALE"]
    IX --> CPI[SPL transfer\nliquidity_vault → user\nauthority = lending_market PDA]
```

---

## Fixed-Point Math Reference

```
RATE_SCALE = 1_000_000_000_000  (1e12)

Period rate (per slot, scaled):
  period_rate = borrow_rate_bps × slots_elapsed × RATE_SCALE
                ─────────────────────────────────────────────
                        SLOTS_PER_YEAR × BPS_SCALER

Debt accrual on Reserve:
  new_debt_sf = old_debt_sf + old_debt_sf × period_rate / RATE_SCALE

Cumulative rate index growth:
  new_rate_sf = old_rate_sf + old_rate_sf × period_rate / RATE_SCALE

Debt catch-up on Obligation (at refresh):
  new_debt_sf = old_debt_sf × current_rate_sf / stored_rate_sf
```
