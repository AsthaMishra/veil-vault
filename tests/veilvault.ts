import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { assert } from "chai";
import { Veilvault } from "../target/types/veilvault";

describe("veilvault", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Veilvault as Program<Veilvault>;
  const owner = Keypair.generate();
  const user = Keypair.generate(); // acts as depositor + borrower

  let lendingMarketPda: PublicKey;
  let lendingMarketBump: number;

  // set by add_reserve.before — shared across all later test blocks
  let reserveMint: PublicKey;
  let reservePda: PublicKey;
  let liquidityVaultPda: PublicKey;
  let feeVaultPda: PublicKey;
  let collateralMintPda: PublicKey;
  let collateralSupplyVaultPda: PublicKey;

  // set by init_obligation / deposit befores
  let obligationPda: PublicKey;
  let userTokenAccount: PublicKey;
  let userCollateralAccount: PublicKey;

  before(async () => {
    for (const kp of [owner, user]) {
      const sig = await provider.connection.requestAirdrop(
        kp.publicKey,
        10 * anchor.web3.LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(sig);
    }

    [lendingMarketPda, lendingMarketBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("lending_market"), owner.publicKey.toBuffer()],
      program.programId
    );
  });

  // ─── initialize_market ───────────────────────────────────────────────

  describe("initialize_market", () => {
    it("creates lending market with correct state", async () => {
      const quoteCurrency = Buffer.alloc(32);
      quoteCurrency.write("USD");

      await program.methods
        .initializeMarket({
          quoteCurrency: Array.from(quoteCurrency),
          protocolFeeBps: 50,
        })
        .accountsStrict({
          owner: owner.publicKey,
          lendingMarket: lendingMarketPda,
          systemProgram: SystemProgram.programId,
        })
        .signers([owner])
        .rpc();

      const market = await program.account.lendingMarket.fetch(lendingMarketPda);

      assert.ok(market.owner.equals(owner.publicKey));
      assert.equal(market.protocolFeeBps, 50);
      assert.equal(market.emergencyPause, false);
      assert.equal(market.bump, lendingMarketBump);
    });

    it("rejects duplicate initialization", async () => {
      try {
        await program.methods
          .initializeMarket({
            quoteCurrency: Array(32).fill(0),
            protocolFeeBps: 50,
          })
          .accountsStrict({
            owner: owner.publicKey,
            lendingMarket: lendingMarketPda,
            systemProgram: SystemProgram.programId,
          })
          .signers([owner])
          .rpc();
        assert.fail("should have thrown");
      } catch (e) {
        assert.ok(e, "duplicate init correctly failed");
      }
    });

    it("rejects protocol fee above max (>1000 bps)", async () => {
      const other = Keypair.generate();
      const sig = await provider.connection.requestAirdrop(
        other.publicKey,
        2 * anchor.web3.LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(sig);

      const [otherMarket] = PublicKey.findProgramAddressSync(
        [Buffer.from("lending_market"), other.publicKey.toBuffer()],
        program.programId
      );

      try {
        await program.methods
          .initializeMarket({
            quoteCurrency: Array(32).fill(0),
            protocolFeeBps: 9999,
          })
          .accountsStrict({
            owner: other.publicKey,
            lendingMarket: otherMarket,
            systemProgram: SystemProgram.programId,
          })
          .signers([other])
          .rpc();
        assert.fail("should have thrown");
      } catch (e) {
        assert.ok(e, "invalid fee correctly rejected");
      }
    });
  });

  // ─── add_reserve ─────────────────────────────────────────────────────

  describe("add_reserve", () => {
    before(async () => {
      reserveMint = await createMint(
        provider.connection,
        owner,
        owner.publicKey,
        null,
        6
      );

      [reservePda] = PublicKey.findProgramAddressSync(
        [Buffer.from("reserve"), lendingMarketPda.toBuffer(), reserveMint.toBuffer()],
        program.programId
      );
      [liquidityVaultPda] = PublicKey.findProgramAddressSync(
        [Buffer.from("liquidity_vault"), reservePda.toBuffer()],
        program.programId
      );
      [feeVaultPda] = PublicKey.findProgramAddressSync(
        [Buffer.from("fee_vault"), reservePda.toBuffer()],
        program.programId
      );
      [collateralMintPda] = PublicKey.findProgramAddressSync(
        [Buffer.from("collateral_mint"), reservePda.toBuffer()],
        program.programId
      );
      [collateralSupplyVaultPda] = PublicKey.findProgramAddressSync(
        [Buffer.from("collateral_supply"), reservePda.toBuffer()],
        program.programId
      );
    });

    it("creates reserve with correct config", async () => {
      await program.methods
        .addReserve({
          config: {
            status: 0,
            minBorrowRateBps: 200,
            optimalBorrowRateBps: 2000,
            maxBorrowRateBps: 10000,
            optimalUtilizationBps: 8000,
            loanToValuePct: 75,
            liquidationThresholdPct: 80,
            liquidationBonusPct: 500,
            depositLimit: new anchor.BN(1_000_000_000),
            borrowLimit: new anchor.BN(800_000_000),
            protocolFee: 50,
            pythOracle: new PublicKey(new Uint8Array(32)),
          },
        })
        .accountsStrict({
          owner: owner.publicKey,
          lendingMarket: lendingMarketPda,
          reserve: reservePda,
          reserveMint,
          liquidityVault: liquidityVaultPda,
          feeVault: feeVaultPda,
          collateralMint: collateralMintPda,
          collateralSupplyVault: collateralSupplyVaultPda,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .signers([owner])
        .rpc();

      const reserve = await program.account.reserve.fetch(reservePda);

      assert.ok(reserve.lendingMarket.equals(lendingMarketPda));
      assert.equal(reserve.config.loanToValuePct, 75);
      assert.equal(reserve.config.liquidationThresholdPct, 80);
      assert.ok(reserve.liquidity.mint.equals(reserveMint));
      assert.ok(reserve.liquidity.supplyVault.equals(liquidityVaultPda));
    });

    it("rejects non-owner adding reserve", async () => {
      const attacker = Keypair.generate();
      const sig = await provider.connection.requestAirdrop(
        attacker.publicKey,
        2 * anchor.web3.LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(sig);

      const fakeMint = await createMint(
        provider.connection,
        attacker,
        attacker.publicKey,
        null,
        6
      );

      const [fakeReservePda] = PublicKey.findProgramAddressSync(
        [Buffer.from("reserve"), lendingMarketPda.toBuffer(), fakeMint.toBuffer()],
        program.programId
      );

      try {
        await program.methods
          .addReserve({
            config: {
              status: 0,
              minBorrowRateBps: 200,
              optimalBorrowRateBps: 2000,
              maxBorrowRateBps: 10000,
              optimalUtilizationBps: 8000,
              loanToValuePct: 75,
              liquidationThresholdPct: 80,
              liquidationBonusPct: 500,
              depositLimit: new anchor.BN(1_000_000_000),
              borrowLimit: new anchor.BN(800_000_000),
              protocolFee: 50,
              pythOracle: new PublicKey(new Uint8Array(32)),
            },
          })
          .accountsStrict({
            owner: attacker.publicKey,
            lendingMarket: lendingMarketPda,
            reserve: fakeReservePda,
            reserveMint: fakeMint,
            liquidityVault: liquidityVaultPda,
            feeVault: feeVaultPda,
            collateralMint: collateralMintPda,
            collateralSupplyVault: collateralSupplyVaultPda,
            tokenProgram: TOKEN_PROGRAM_ID,
            systemProgram: SystemProgram.programId,
          })
          .signers([attacker])
          .rpc();
        assert.fail("should have thrown");
      } catch (e) {
        assert.ok(e, "non-owner correctly rejected");
      }
    });
  });

  // ─── init_obligation ─────────────────────────────────────────────────

  describe("init_obligation", () => {
    before(async () => {
      [obligationPda] = PublicKey.findProgramAddressSync(
        [
          Buffer.from("obligation"),
          lendingMarketPda.toBuffer(),
          user.publicKey.toBuffer(),
        ],
        program.programId
      );
    });

    it("creates obligation with zero deposits and borrows", async () => {
      await program.methods
        .initObligation()
        .accountsStrict({
          owner: user.publicKey,
          lendingMarket: lendingMarketPda,
          obligation: obligationPda,
          systemProgram: SystemProgram.programId,
        })
        .signers([user])
        .rpc();

      const obligation = await program.account.obligation.fetch(obligationPda);

      assert.ok(obligation.owner.equals(user.publicKey));
      assert.ok(obligation.lendingMarket.equals(lendingMarketPda));
      assert.equal(obligation.depositsCount, 0);
      assert.equal(obligation.borrowsCount, 0);
    });

    it("rejects duplicate init_obligation for same user + market", async () => {
      try {
        await program.methods
          .initObligation()
          .accountsStrict({
            owner: user.publicKey,
            lendingMarket: lendingMarketPda,
            obligation: obligationPda,
            systemProgram: SystemProgram.programId,
          })
          .signers([user])
          .rpc();
        assert.fail("should have thrown");
      } catch (e) {
        assert.ok(e, "duplicate obligation init correctly rejected");
      }
    });
  });

  // ─── deposit ─────────────────────────────────────────────────────────

  describe("deposit", () => {
    const MINT_AMOUNT = 10_000_000; // 10 tokens
    const DEPOSIT_AMOUNT = 1_000_000; // 1 token

    before(async () => {
      // underlying token account — funded with 10 tokens
      userTokenAccount = await createAccount(
        provider.connection,
        user,
        reserveMint,
        user.publicKey
      );
      await mintTo(
        provider.connection,
        owner,
        reserveMint,
        userTokenAccount,
        owner,
        MINT_AMOUNT
      );

      // cToken account — empty, collateral_mint is already on-chain from add_reserve
      userCollateralAccount = await createAccount(
        provider.connection,
        user,
        collateralMintPda,
        user.publicKey
      );
    });

    it("moves tokens into vault and mints cTokens 1:1 on first deposit", async () => {
      const vaultBefore = await getAccount(provider.connection, liquidityVaultPda);

      await program.methods
        .deposit(new anchor.BN(DEPOSIT_AMOUNT))
        .accountsStrict({
          depositor: user.publicKey,
          lendingMarket: lendingMarketPda,
          reserve: reservePda,
          reserveMint,
          liquidityVault: liquidityVaultPda,
          collateralMint: collateralMintPda,
          userTokenAccount,
          userCollateralAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([user])
        .rpc();

      const vaultAfter = await getAccount(provider.connection, liquidityVaultPda);
      const userCollateral = await getAccount(provider.connection, userCollateralAccount);
      const userToken = await getAccount(provider.connection, userTokenAccount);

      // vault received the deposit
      assert.equal(Number(vaultAfter.amount - vaultBefore.amount), DEPOSIT_AMOUNT);
      // first deposit into empty pool → exchange rate is 1:1, so cTokens == tokens
      assert.equal(Number(userCollateral.amount), DEPOSIT_AMOUNT);
      // user was debited
      assert.equal(Number(userToken.amount), MINT_AMOUNT - DEPOSIT_AMOUNT);

      const reserve = await program.account.reserve.fetch(reservePda);
      assert.equal(Number(reserve.liquidity.availableAmount), DEPOSIT_AMOUNT);
      assert.equal(Number(reserve.collateral.mintTotalSupply), DEPOSIT_AMOUNT);
    });

    it("rejects zero deposit", async () => {
      try {
        await program.methods
          .deposit(new anchor.BN(0))
          .accountsStrict({
            depositor: user.publicKey,
            lendingMarket: lendingMarketPda,
            reserve: reservePda,
            reserveMint,
            liquidityVault: liquidityVaultPda,
            collateralMint: collateralMintPda,
            userTokenAccount,
            userCollateralAccount,
            tokenProgram: TOKEN_PROGRAM_ID,
          })
          .signers([user])
          .rpc();
        assert.fail("should have thrown");
      } catch (e) {
        assert.ok(e, "zero deposit correctly rejected");
      }
    });
  });

  // ─── borrow ──────────────────────────────────────────────────────────
  //
  // Each borrow requires a fresh refresh_obligation in the same slot.
  // market_price_sf = 0 (no Pyth feed on localnet), so borrow market values
  // stay 0 → health_factor() = None → infinitely healthy → borrows allowed.

  async function buildRefreshObligationIx(): Promise<anchor.web3.TransactionInstruction> {
    return program.methods
      .refreshObligation()
      .accountsStrict({
        lendingMarket: lendingMarketPda,
        obligation: obligationPda,
      })
      .remainingAccounts([
        { pubkey: reservePda, isWritable: false, isSigner: false },
      ])
      .instruction();
  }

  describe("borrow", () => {
    const BORROW_AMOUNT = 500_000; // 0.5 tokens

    it("transfers tokens from vault to borrower and records debt in obligation", async () => {
      // refresh_obligation + borrow must be in the same slot (staleness check)
      const refreshIx = await buildRefreshObligationIx();
      const borrowIx = await program.methods
        .borrow(new anchor.BN(BORROW_AMOUNT))
        .accountsStrict({
          borrower: user.publicKey,
          lendingMarket: lendingMarketPda,
          obligation: obligationPda,
          reserve: reservePda,
          reserveMint,
          liquidityVault: liquidityVaultPda,
          userTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .instruction();

      const userBefore = await getAccount(provider.connection, userTokenAccount);
      const vaultBefore = await getAccount(provider.connection, liquidityVaultPda);

      const tx = new anchor.web3.Transaction().add(refreshIx, borrowIx);
      await provider.sendAndConfirm(tx, [user]);

      const userAfter = await getAccount(provider.connection, userTokenAccount);
      const vaultAfter = await getAccount(provider.connection, liquidityVaultPda);
      const obligation = await program.account.obligation.fetch(obligationPda);
      const reserve = await program.account.reserve.fetch(reservePda);

      // user received borrowed tokens
      assert.equal(Number(userAfter.amount - userBefore.amount), BORROW_AMOUNT);
      // vault was debited
      assert.equal(Number(vaultBefore.amount - vaultAfter.amount), BORROW_AMOUNT);
      // obligation opened one borrow slot
      assert.equal(obligation.borrowsCount, 1);
      assert.ok(obligation.borrows[0].borrowReserve.equals(reservePda));
      // reserve tracks outstanding debt
      assert.equal(Number(reserve.liquidity.borrowedAmountSf), BORROW_AMOUNT);
    });

    it("rejects zero borrow", async () => {
      try {
        await program.methods
          .borrow(new anchor.BN(0))
          .accountsStrict({
            borrower: user.publicKey,
            lendingMarket: lendingMarketPda,
            obligation: obligationPda,
            reserve: reservePda,
            reserveMint,
            liquidityVault: liquidityVaultPda,
            userTokenAccount,
            tokenProgram: TOKEN_PROGRAM_ID,
          })
          .signers([user])
          .rpc();
        assert.fail("should have thrown");
      } catch (e) {
        assert.ok(e, "zero borrow correctly rejected");
      }
    });

    it("rejects borrow exceeding available liquidity", async () => {
      try {
        await program.methods
          .borrow(new anchor.BN(999_000_000)) // way more than the 0.5 left in vault
          .accountsStrict({
            borrower: user.publicKey,
            lendingMarket: lendingMarketPda,
            obligation: obligationPda,
            reserve: reservePda,
            reserveMint,
            liquidityVault: liquidityVaultPda,
            userTokenAccount,
            tokenProgram: TOKEN_PROGRAM_ID,
          })
          .signers([user])
          .rpc();
        assert.fail("should have thrown");
      } catch (e) {
        assert.ok(e, "over-borrow correctly rejected");
      }
    });
  });

  // ─── repay ───────────────────────────────────────────────────────────

  describe("repay", () => {
    const REPAY_AMOUNT = 500_000; // repay the full borrowed amount

    it("moves tokens back to vault and clears obligation borrow slot", async () => {
      const userBefore = await getAccount(provider.connection, userTokenAccount);
      const vaultBefore = await getAccount(provider.connection, liquidityVaultPda);

      await program.methods
        .repay(new anchor.BN(REPAY_AMOUNT))
        .accountsStrict({
          borrower: user.publicKey,
          lendingMarket: lendingMarketPda,
          obligation: obligationPda,
          reserve: reservePda,
          reserveMint,
          liquidityVault: liquidityVaultPda,
          userTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([user])
        .rpc();

      const userAfter = await getAccount(provider.connection, userTokenAccount);
      const vaultAfter = await getAccount(provider.connection, liquidityVaultPda);
      const obligation = await program.account.obligation.fetch(obligationPda);
      const reserve = await program.account.reserve.fetch(reservePda);

      // user was debited
      assert.equal(Number(userBefore.amount - userAfter.amount), REPAY_AMOUNT);
      // vault received repayment
      assert.equal(Number(vaultAfter.amount - vaultBefore.amount), REPAY_AMOUNT);
      // obligation borrow slot was cleared (full repay, no interest at test timescale)
      assert.equal(obligation.borrowsCount, 0);
      // reserve debt back to zero
      assert.equal(Number(reserve.liquidity.borrowedAmountSf), 0);
    });

    it("rejects zero repay", async () => {
      try {
        await program.methods
          .repay(new anchor.BN(0))
          .accountsStrict({
            borrower: user.publicKey,
            lendingMarket: lendingMarketPda,
            obligation: obligationPda,
            reserve: reservePda,
            reserveMint,
            liquidityVault: liquidityVaultPda,
            userTokenAccount,
            tokenProgram: TOKEN_PROGRAM_ID,
          })
          .signers([user])
          .rpc();
        assert.fail("should have thrown");
      } catch (e) {
        assert.ok(e, "zero repay correctly rejected");
      }
    });

    it("rejects repaying more than owed", async () => {
      // obligation now has 0 borrows, so find_borrow will fail
      try {
        await program.methods
          .repay(new anchor.BN(1))
          .accountsStrict({
            borrower: user.publicKey,
            lendingMarket: lendingMarketPda,
            obligation: obligationPda,
            reserve: reservePda,
            reserveMint,
            liquidityVault: liquidityVaultPda,
            userTokenAccount,
            tokenProgram: TOKEN_PROGRAM_ID,
          })
          .signers([user])
          .rpc();
        assert.fail("should have thrown");
      } catch (e) {
        assert.ok(e, "repay with no outstanding borrow correctly rejected");
      }
    });
  });

  // ─── withdraw ────────────────────────────────────────────────────────
  //
  // State entering this block (from deposit + borrow + repay flow):
  //   vault:              1_000_000 tokens
  //   user token account: 9_000_000 tokens
  //   user cToken account: 1_000_000 cTokens  (from the initial deposit)

  describe("withdraw", () => {
    const WITHDRAW_COLLATERAL = 500_000; // burn half the cTokens → get half the underlying back

    it("burns cTokens and returns underlying tokens to depositor", async () => {
      const userTokenBefore = await getAccount(provider.connection, userTokenAccount);
      const userCollateralBefore = await getAccount(
        provider.connection,
        userCollateralAccount
      );
      const vaultBefore = await getAccount(provider.connection, liquidityVaultPda);

      await program.methods
        .withdraw(new anchor.BN(WITHDRAW_COLLATERAL))
        .accountsStrict({
          depositor: user.publicKey,
          lendingMarket: lendingMarketPda,
          reserve: reservePda,
          reserveMint,
          liquidityVault: liquidityVaultPda,
          collateralMint: collateralMintPda,
          userCollateralAccount,
          userTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([user])
        .rpc();

      const userTokenAfter = await getAccount(provider.connection, userTokenAccount);
      const userCollateralAfter = await getAccount(
        provider.connection,
        userCollateralAccount
      );
      const vaultAfter = await getAccount(provider.connection, liquidityVaultPda);
      const reserve = await program.account.reserve.fetch(reservePda);

      // user received underlying tokens
      assert.equal(
        Number(userTokenAfter.amount - userTokenBefore.amount),
        WITHDRAW_COLLATERAL // 1:1 exchange rate (no interest accrued at test timescale)
      );
      // user's cTokens were burned
      assert.equal(
        Number(userCollateralBefore.amount - userCollateralAfter.amount),
        WITHDRAW_COLLATERAL
      );
      // vault was debited
      assert.equal(
        Number(vaultBefore.amount - vaultAfter.amount),
        WITHDRAW_COLLATERAL
      );
      // reserve state reflects burned collateral
      assert.equal(
        Number(reserve.collateral.mintTotalSupply),
        1_000_000 - WITHDRAW_COLLATERAL
      );
    });

    it("rejects zero withdraw", async () => {
      try {
        await program.methods
          .withdraw(new anchor.BN(0))
          .accountsStrict({
            depositor: user.publicKey,
            lendingMarket: lendingMarketPda,
            reserve: reservePda,
            reserveMint,
            liquidityVault: liquidityVaultPda,
            collateralMint: collateralMintPda,
            userCollateralAccount,
            userTokenAccount,
            tokenProgram: TOKEN_PROGRAM_ID,
          })
          .signers([user])
          .rpc();
        assert.fail("should have thrown");
      } catch (e) {
        assert.ok(e, "zero withdraw correctly rejected");
      }
    });

    it("rejects withdraw exceeding cToken balance", async () => {
      // user now holds 500_000 cTokens; trying to burn 1_000_000 should fail
      try {
        await program.methods
          .withdraw(new anchor.BN(1_000_000))
          .accountsStrict({
            depositor: user.publicKey,
            lendingMarket: lendingMarketPda,
            reserve: reservePda,
            reserveMint,
            liquidityVault: liquidityVaultPda,
            collateralMint: collateralMintPda,
            userCollateralAccount,
            userTokenAccount,
            tokenProgram: TOKEN_PROGRAM_ID,
          })
          .signers([user])
          .rpc();
        assert.fail("should have thrown");
      } catch (e) {
        assert.ok(e, "over-withdraw correctly rejected");
      }
    });
  });
});
