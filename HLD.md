# VeilVault — High Level Diagram

## System Components

```mermaid
graph TD
    subgraph Solana Program ["VeilVault Program (on-chain)"]
        LM[LendingMarket\nglobal config · pause · fees]
        R1[Reserve USDC\nliquidity · interest · cToken]
        R2[Reserve SOL\nliquidity · interest · cToken]
        OB[Obligation\nper-user · deposits · borrows]
        LM --> R1
        LM --> R2
        R1 --> OB
        R2 --> OB
    end

    subgraph External
        PYTH[Pyth Oracle\nprice feeds]
        ARCIUM[Arcium MPC\nconfidential compute]
        SPL[SPL Token Program\nmint · transfer · burn]
    end

    subgraph Users
        DEP[Depositor]
        BOR[Borrower]
        LIQ[Liquidator]
        GOV[Market Owner]
    end

    DEP -- deposit / withdraw --> R1
    BOR -- borrow / repay --> OB
    LIQ -- liquidate --> OB
    GOV -- add_reserve\npause\nupdate_config --> LM

    PYTH -- price + staleness --> OB
    ARCIUM -- encrypted HF check --> OB
    SPL -- token CPI --> R1
    SPL -- cToken mint/burn CPI --> R1
```

---

## User Flows

### Deposit → Borrow → Repay

```mermaid
sequenceDiagram
    actor User
    participant IX as Instruction
    participant Res as Reserve
    participant Obl as Obligation
    participant Vault as Liquidity Vault
    participant cMint as cToken Mint

    User->>IX: deposit(amount)
    IX->>Res: deposit_liquidity(amount)
    Res-->>IX: collateral_amount (cTokens to mint)
    IX->>Vault: SPL transfer (user → vault)
    IX->>cMint: SPL mint_to (user wallet)

    User->>IX: borrow(amount)
    IX->>Obl: borrow(reserve, amount, current_rate)
    IX->>Res: borrow(amount)
    IX->>Vault: SPL transfer (vault → user)

    User->>IX: repay(amount)
    IX->>Vault: SPL transfer (user → vault)
    IX->>Res: repay(amount)
    IX->>Obl: repay(reserve, amount)
```

### Liquidation

```mermaid
sequenceDiagram
    actor Liquidator
    participant IX as liquidate
    participant Obl as Obligation
    participant DebtRes as Debt Reserve
    participant ColRes as Collateral Reserve

    Liquidator->>IX: liquidate(obligation, repay_amount)
    IX->>Obl: check HF < 1.0
    IX->>DebtRes: repay(repay_amount)
    IX->>Obl: repay(debt_reserve, repay_amount)
    IX->>ColRes: compute bonus_collateral = repay_value × (1 + bonus_pct)
    IX->>Obl: withdraw(collateral_reserve, bonus_collateral)
    IX->>Liquidator: transfer bonus_collateral cTokens
```

### Interest Accrual

```mermaid
flowchart LR
    A[refresh_reserve\ncalled each slot] --> B[compute borrow_rate\nfrom utilization]
    B --> C[accrued_interest\ndebt += debt × period_rate\nfees += debt × protocol_fee\ncumulative_rate_sf grows]
    C --> D[refresh_obligation\nper borrow slot]
    D --> E["new_debt = old_debt\n× current_rate / stored_rate"]
    E --> F[snapshot stored_rate\n= current_rate]
```

---

## Account Relationships

```mermaid
erDiagram
    LendingMarket ||--o{ Reserve : "owns"
    LendingMarket ||--o{ Obligation : "scopes"
    Reserve ||--|| LiquidityVault : "holds tokens"
    Reserve ||--|| FeeVault : "holds protocol fees"
    Reserve ||--|| CollateralMint : "mints cTokens"
    Obligation ||--o{ ObligationCollateral : "up to 8 deposits"
    Obligation ||--o{ ObligationLiquidity : "up to 8 borrows"
    ObligationCollateral }o--|| Reserve : "deposit_reserve ref"
    ObligationLiquidity }o--|| Reserve : "borrow_reserve ref"
```

---

## Privacy Extension (Arcium)

```mermaid
flowchart TD
    subgraph Standard Flow
        A[User deposits\nplaintext amount]
        B[Obligation stores\nplaintext cTokens]
        C[HF computed\non-chain plaintext]
    end

    subgraph Private Flow
        D[User deposits\nencrypted amount]
        E[Obligation stores\nciphertext]
        F[HF check via\nArcium MPC]
        G[MPC outputs\nbool: is_liquidatable]
    end

    A -.->|Arcium layer replaces| D
    B -.-> E
    C -.-> F
    F --> G
```
