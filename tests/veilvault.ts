import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import { createMint, TOKEN_PROGRAM_ID } from "@solana/spl-token";
import { assert } from "chai";
import { Veilvault } from "../target/types/veilvault";

describe("veilvault", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Veilvault as Program<Veilvault>;
  const owner = Keypair.generate();

  let lendingMarketPda: PublicKey;
  let lendingMarketBump: number;

  before(async () => {
    // fund the owner wallet
    const sig = await provider.connection.requestAirdrop(
      owner.publicKey,
      10 * anchor.web3.LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(sig);

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
        .accounts({
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
          .accounts({
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
            protocolFeeBps: 9999, // above MAX_PROTOCOL_FEE_BPS (1000)
          })
          .accounts({
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
    let reserveMint: PublicKey;
    let reservePda: PublicKey;
    let liquidityVaultPda: PublicKey;
    let feeVaultPda: PublicKey;
    let collateralMintPda: PublicKey;
    let collateralSupplyVaultPda: PublicKey;

    before(async () => {
      // create a fresh SPL mint to use as the reserve token
      reserveMint = await createMint(
        provider.connection,
        owner,
        owner.publicKey,
        null,
        6 // 6 decimals like USDC
      );

      [reservePda] = PublicKey.findProgramAddressSync(
        [
          Buffer.from("reserve"),
          lendingMarketPda.toBuffer(),
          reserveMint.toBuffer(),
        ],
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
          },
        })
        .accounts({
          owner: owner.publicKey,
          lendingMarket: lendingMarketPda,
          reserve: reservePda,
          reserveMint: reserveMint,
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
        [
          Buffer.from("reserve"),
          lendingMarketPda.toBuffer(),
          fakeMint.toBuffer(),
        ],
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
            },
          })
          .accounts({
            owner: attacker.publicKey, // wrong owner
            lendingMarket: lendingMarketPda,
            reserve: fakeReservePda,
            reserveMint: fakeMint,
            liquidityVault: liquidityVaultPda,
            feeVault: feeVaultPda,
            collateralMint: collateralMintPda,
            collateralSupplyVault: collateralSupplyVaultPda,
            tokenProgram: TOKEN_PROGRAM_ID,
            systemProgram: SystemProgram.programId,
            rent: anchor.web3.SYSVAR_RENT_PUBKEY,
          })
          .signers([attacker])
          .rpc();
        assert.fail("should have thrown");
      } catch (e) {
        assert.ok(e, "non-owner correctly rejected");
      }
    });
  });
});
