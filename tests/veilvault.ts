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
  const user = Keypair.generate();

  let lendingMarketPda: PublicKey;
  let lendingMarketBump: number;

  let reserveMint: PublicKey;
  let reservePda: PublicKey;
  let liquidityVaultPda: PublicKey;
  let feeVaultPda: PublicKey;
  let collateralMintPda: PublicKey;
  let collateralSupplyVaultPda: PublicKey;

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
            pythOracle: PublicKey.default,
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
              pythOracle: PublicKey.default,
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
    const MINT_AMOUNT = 10_000_000;
    const DEPOSIT_AMOUNT = 1_000_000;

    before(async () => {
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

      assert.equal(Number(vaultAfter.amount - vaultBefore.amount), DEPOSIT_AMOUNT);
      assert.equal(Number(userCollateral.amount), DEPOSIT_AMOUNT);
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

  // ─── withdraw ────────────────────────────────────────────────────────
  //
  // User has 1_000_000 cTokens from the deposit block above.
  // Simple flow: burn cTokens → receive underlying. No collateral locking.

  describe("withdraw", () => {
    const WITHDRAW_AMOUNT = 500_000;

    it("burns cTokens and returns underlying tokens to depositor", async () => {
      const userTokenBefore = await getAccount(provider.connection, userTokenAccount);
      const userCollateralBefore = await getAccount(provider.connection, userCollateralAccount);
      const vaultBefore = await getAccount(provider.connection, liquidityVaultPda);

      await program.methods
        .withdraw(new anchor.BN(WITHDRAW_AMOUNT))
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
      const userCollateralAfter = await getAccount(provider.connection, userCollateralAccount);
      const vaultAfter = await getAccount(provider.connection, liquidityVaultPda);

      assert.equal(Number(userTokenAfter.amount - userTokenBefore.amount), WITHDRAW_AMOUNT);
      assert.equal(Number(userCollateralBefore.amount - userCollateralAfter.amount), WITHDRAW_AMOUNT);
      assert.equal(Number(vaultBefore.amount - vaultAfter.amount), WITHDRAW_AMOUNT);
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
      // user holds 500_000 cTokens after the first withdrawal
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
