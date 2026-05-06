# VeilVault — Architecture

## Layers

```mermaid
flowchart TB
    subgraph Client ["Client Layer"]
        TS[TypeScript SDK / Tests]
        UI[Next.js Frontend]
    end

    subgraph Anchor ["Anchor Program (veilvault)"]
        direction TB
        subgraph Instructions ["instructions/"]
            I1[initialize_market]
            I2[add_reserve]
            I3[deposit]
            I4[borrow]
            I5[repay / withdraw]
            I6[refresh_reserve\nrefresh_obligation]
            I7[liquidate]
            I8[pause / update_config]
        end
        subgraph State ["state/"]
            S1[LendingMarket]
            S2[Reserve\nReserveConfig\nReserveLiquidity\nReserveCollateral]
            S3[Obligation\nObligationCollateral\nObligationLiquidity]
        end
        subgraph Utils ["utils/"]
            U1[LastUpdate\nstaleness checks]
        end
    end

    subgraph Runtime ["Solana Runtime"]
        SPL[SPL Token Program]
        SYS[System Program]
    end

    subgraph Oracle ["Oracle Layer"]
        PYTH[Pyth Network]
    end

    subgraph Privacy ["Privacy Layer"]
        MPC[Arcium MXE\nMPC Network]
    end

    Client --> Instructions
    Instructions --> State
    Instructions --> Utils
    Instructions -- CPI --> SPL
    Instructions -- CPI --> SYS
    I6 --> PYTH
    I7 --> MPC
```

---

## Component Responsibilities

```mermaid
flowchart LR
    subgraph instructions ["Instructions — what each one owns"]
        direction TB
        A["Account validation\n(Anchor constraints)"]
        B["Authorization checks\n(owner, signer, has_one)"]
        C["CPI calls\n(SPL token transfers,\nmint, burn)"]
        D["Glue: call state methods\nin the right order"]
    end

    subgraph state ["State — what structs own"]
        direction TB
        E["Pure business logic\n(no Anchor, no CPI)"]
        F["Checked arithmetic\n(no raw +/-)"]
        G["Invariant enforcement\n(limits, LTV, borrow_rate)"]
        H["Fixed-point math\n(RATE_SCALE, _sf fields)"]
    end

    instructions -- "calls methods on" --> state
```

---

## Account Authority Map

```mermaid
flowchart TD
    LM["LendingMarket PDA\n(seeds: lending_market + owner)"]

    LM -- "authority over" --> LV["Liquidity Vault\n(token account)"]
    LM -- "authority over" --> FV["Fee Vault\n(token account)"]
    LM -- "authority over" --> CM["Collateral Mint\n(SPL mint)"]

    LM -- "checked via has_one" --> OWNER["Market Owner\n(governance key)"]

    USER["User Wallet"] -- "signs" --> OBL["Obligation PDA\n(seeds: obligation + market + user)"]
```

---

## Instruction Permission Matrix

```
Instruction          │ Anyone │ Owner only │ Paused market blocks
─────────────────────┼────────┼────────────┼─────────────────────
initialize_market    │        │     ✓      │
add_reserve          │        │     ✓      │       ✓
deposit              │   ✓    │            │       ✓
borrow               │   ✓    │            │       ✓
repay                │   ✓    │            │
withdraw             │   ✓    │            │       ✓
refresh_reserve      │   ✓    │            │
refresh_obligation   │   ✓    │            │
liquidate            │   ✓    │            │       ✓
pause / unpause      │        │     ✓      │
update_reserve_config│        │     ✓      │
```

---

## Data Flow — Full Deposit Lifecycle

```mermaid
stateDiagram-v2
    [*] --> Idle : user has USDC

    Idle --> Deposited : deposit(amount)\nSPL transfer user→vault\ncTokens minted to user

    Deposited --> Borrowed : borrow(amount)\ncheck HF ≥ 1.0\nSPL transfer vault→user

    Borrowed --> InterestAccruing : slots pass\nrefresh_reserve grows\ncumulative_borrow_rate_sf

    InterestAccruing --> Refreshed : refresh_obligation\ndebt catches up via\nnew = old × rate_now / rate_then

    Refreshed --> Repaid : repay(amount)\nSPL transfer user→vault\ndebt reduced

    Repaid --> Withdrawn : withdraw(ctokens)\ncheck HF ≥ 1.0\ncTokens burned\nSPL transfer vault→user

    Borrowed --> Liquidatable : HF drops below 1.0\nCollateral value fell or\ndebt grew too large

    Liquidatable --> PartiallyLiquidated : liquidate(repay_amount)\nliquidator repays debt\nreceives collateral + bonus

    PartiallyLiquidated --> Refreshed : if HF restored
    PartiallyLiquidated --> Liquidatable : if HF still < 1.0

    Withdrawn --> [*]
    Repaid --> [*]
```

---

## Cross-Program Invocations

```mermaid
sequenceDiagram
    participant IX as VeilVault Instruction
    participant SPL as SPL Token Program
    participant SYS as System Program

    Note over IX,SYS: add_reserve (account creation)
    IX->>SYS: create_account (reserve PDA)
    IX->>SPL: initialize_account (liquidity_vault)
    IX->>SPL: initialize_account (fee_vault)
    IX->>SPL: initialize_mint (collateral_mint)

    Note over IX,SPL: deposit
    IX->>SPL: transfer(user_token_account → liquidity_vault)
    IX->>SPL: mint_to(collateral_mint → user_ctoken_account)\nsigner: lending_market PDA

    Note over IX,SPL: borrow
    IX->>SPL: transfer(liquidity_vault → user_token_account)\nsigner: lending_market PDA

    Note over IX,SPL: withdraw
    IX->>SPL: burn(user_ctoken_account, collateral_mint)
    IX->>SPL: transfer(liquidity_vault → user_token_account)\nsigner: lending_market PDA
```
