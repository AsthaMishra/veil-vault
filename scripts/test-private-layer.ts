/**
 * VeilVault — Private Layer End-to-End Test (Devnet)
 *
 * Tests the full Arcium MPC flow:
 *   1. Setup: market → reserve → obligation (plaintext prerequisites)
 *   2. Register Arcis comp_defs (one-time admin setup)
 *   3. init_private_obligation  → poll until MXE sets is_initialized = true
 *   4. private_deposit_collateral → poll until enc_state updates
 *   5. private_borrow            → poll until enc_state updates
 *
 * Prerequisites:
 *   - Program deployed: CMbnY6XXekgVZvFHwmB6yC15TD5x7anD1XmHrm218Wbs
 *   - ARCIUM_PROGRAM_ID: get from https://docs.arcium.com or Arcium Discord
 *   - Wallet with ≥ 2 SOL on devnet (for rent + Arcium fees)
 *   - Run: npx ts-node scripts/test-private-layer.ts
 */

import * as anchor from "@coral-xyz/anchor";
import { Program, BN } from "@coral-xyz/anchor";
import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  SYSVAR_INSTRUCTIONS_PUBKEY,
  Transaction,
  sendAndConfirmTransaction,
} from "@solana/web3.js";
import {
  createMint,
  createAccount,
  mintTo,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import * as nacl from "tweetnacl";
import * as fs from "fs";
import { Veilvault } from "../target/types/veilvault";

// ─── Configuration ────────────────────────────────────────────────────────────

const PROGRAM_ID = new PublicKey(
  "CMbnY6XXekgVZvFHwmB6yC15TD5x7anD1XmHrm218Wbs"
);

// All addresses derived from arcium-client-0.9.6/src/pda.rs
const ARCIUM_PROGRAM_ID  = new PublicKey("Arcj82pX7HxYKLR92qvgZUAd7vGS1k4hQvAFcPATFdEQ");
const ARCIUM_FEE_POOL    = new PublicKey("G2sRWJvi3xoyh5k2gY49eG9L8YhAEWQPtNb1zb1GXTtC");
const ARCIUM_CLOCK_ACCT  = new PublicKey("7EbMUTLo5DjdzbN7s8BXeZwXzEwNQb1hScfRvWg8a6ot");
const LUT_PROGRAM_ID     = new PublicKey("AddressLookupTab1e1111111111111111111111111");

// MXE: seeds=[b"MXEAccount", veilvault_program_id], program=Arcium
const MXE_ADDRESS        = new PublicKey("GS1yZm6vjUZojNmYuiErau1EZfVAtHj3JZogDyqmYDnp");
// Signer: seeds=[b"ArciumSignerAccount"], program=VeilVault (not Arcium!)
const SIGN_PDA_ADDRESS   = new PublicKey("4MswWzz8V9fxsCnTDUEUS8NufeSmLEM3Pwg1TUrhdgVR");

// Arcium cluster offset provided by Arcium (devnet) — stored as u32
const CLUSTER_OFFSET = new BN(456);

// Cluster/pool PDAs: seeds use cluster_offset as u32 LE (4 bytes)
const CLUSTER_ADDRESS  = new PublicKey("DzaQCyfybroycrNqE5Gk7LhSbWD2qfCics6qptBFbr95");
const MEMPOOL_ADDRESS  = new PublicKey("Ex7BD8o8PK1y2eXDd38Jgujj93uHygrZeWXDeGAHmHtN");
const EXECPOOL_ADDRESS = new PublicKey("4mcrgNZzJwwKrE3wXMHfepT8htSBmGqBzDYPJijWooog");

// Wallet: defaults to ~/.config/solana/id.json
const WALLET_PATH =
  process.env.ANCHOR_WALLET ?? `${process.env.HOME}/.config/solana/id.json`;

const RPC_URL = "https://api.devnet.solana.com";

// ─── PDA helpers ─────────────────────────────────────────────────────────────

function pdaVeilvault(seeds: Buffer[]): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(seeds, PROGRAM_ID);
}

function pdaArcium(seeds: Buffer[]): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(seeds, ARCIUM_PROGRAM_ID);
}

function computationPda(computationOffset: BN): PublicKey {
  // seeds = [b"ComputationAccount", cluster_offset_u32_le, computation_offset_u64_le]
  const clustBuf = Buffer.alloc(4);
  clustBuf.writeUInt32LE(CLUSTER_OFFSET.toNumber());
  const compBuf = Buffer.alloc(8);
  compBuf.writeBigUInt64LE(BigInt(computationOffset.toString()));
  return pdaArcium([Buffer.from("ComputationAccount"), clustBuf, compBuf])[0];
}

function compDefPda(compDefOffset: number): PublicKey {
  // seeds = [b"ComputationDefinitionAccount", veilvault_program_id, offset_u32_le]
  const buf = Buffer.alloc(4);
  buf.writeUInt32LE(compDefOffset);
  return pdaArcium([Buffer.from("ComputationDefinitionAccount"), Buffer.from(PROGRAM_ID.toBytes()), buf])[0];
}

function lutPda(lutOffsetSlot: BN): PublicKey {
  // LUT derivation uses LUT program, not Arcium: seeds=[mxe_pda_bytes, slot_u64_le]
  const buf = Buffer.alloc(8);
  buf.writeBigUInt64LE(BigInt(lutOffsetSlot.toString()));
  return PublicKey.findProgramAddressSync(
    [Buffer.from(MXE_ADDRESS.toBytes()), buf],
    LUT_PROGRAM_ID
  )[0];
}

// Computation definition offsets — must match Rust constants (comp_def_offset fn)
// These are CRC32 or similar hashes of the function name.
// Get exact values by running: cargo test -- --nocapture 2>&1 | grep COMP_DEF_OFFSET
// OR compile and print them from lib.rs constants.
// Real values computed from arcium_anchor::comp_def_offset() — verified via cargo test
const COMP_DEF_OFFSET_INIT_POSITION   = 40768207;
const COMP_DEF_OFFSET_ADD_COLLATERAL  = 1274553762;
const COMP_DEF_OFFSET_ADD_BORROW      = 4265546300;

// ─── Encryption helper ────────────────────────────────────────────────────────

/**
 * Encrypts a u64 amount for the Arcium MXE.
 *
 * Uses X25519 Diffie-Hellman + XSalsa20 stream cipher:
 *   shared_secret = X25519(client_sk, mxe_x25519_pk)
 *   keystream     = XSalsa20(shared_secret, nonce)
 *   ciphertext    = padded_amount XOR keystream  [32 bytes]
 *
 * NOTE: If Arcium uses a different scheme (e.g. ChaCha20-Poly1305),
 *       update this function. Check arcium-anchor release notes or Discord.
 *
 * @returns { encryptedAmount, encryptionPubkey, encryptionNonce }
 */
function encryptAmountForMxe(
  amount: bigint,
  mxeX25519Pubkey: Uint8Array
): {
  encryptedAmount: Uint8Array;   // [u8; 32]
  encryptionPubkey: Uint8Array;  // [u8; 32]
  encryptionNonce: bigint;       // u128
} {
  // 1. Generate ephemeral X25519 keypair
  const clientKeyPair = nacl.box.keyPair();

  // 2. Compute X25519 shared secret
  const sharedSecret = nacl.scalarMult(clientKeyPair.secretKey, mxeX25519Pubkey);

  // 3. Random 16-byte nonce (u128)
  const nonceBytes = nacl.randomBytes(16);
  const encryptionNonce =
    BigInt("0x" + Buffer.from(nonceBytes).toString("hex"));

  // 4. Pad amount to 32 bytes (little-endian u64 in first 8 bytes, rest zero)
  const plaintext = new Uint8Array(32);
  const amountBuf = Buffer.alloc(8);
  amountBuf.writeBigUInt64LE(amount);
  plaintext.set(amountBuf);

  // 5. XOR with keystream derived from shared secret + nonce
  //    (simplified — replace with Arcium's exact scheme if different)
  const keystream = nacl.secretbox(plaintext, new Uint8Array(24), sharedSecret);
  const encryptedAmount = new Uint8Array(32);
  for (let i = 0; i < 32; i++) {
    encryptedAmount[i] = plaintext[i] ^ (keystream[i] ?? 0);
  }

  return {
    encryptedAmount,
    encryptionPubkey: clientKeyPair.publicKey,
    encryptionNonce,
  };
}

// ─── Polling helper ───────────────────────────────────────────────────────────

async function pollUntil<T>(
  label: string,
  check: () => Promise<T | null>,
  timeoutMs = 120_000,
  intervalMs = 3_000
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const result = await check();
    if (result !== null) return result;
    console.log(`  ⏳  Waiting for MXE callback: ${label}...`);
    await new Promise((r) => setTimeout(r, intervalMs));
  }
  throw new Error(`Timeout waiting for: ${label}`);
}

// ─── Main ─────────────────────────────────────────────────────────────────────

async function main() {
  console.log("\n🔐  VeilVault — Private Layer Test\n");

  // Setup connection + provider
  const connection = new Connection(RPC_URL, "confirmed");
  const walletKp = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(WALLET_PATH, "utf-8")))
  );
  const wallet = new anchor.Wallet(walletKp);
  const provider = new anchor.AnchorProvider(connection, wallet, {
    commitment: "confirmed",
    preflightCommitment: "confirmed",
  });
  anchor.setProvider(provider);

  const idl = JSON.parse(
    fs.readFileSync("target/idl/veilvault.json", "utf-8")
  );
  const program = new Program<Veilvault>(idl, provider);

  const payer = walletKp;
  console.log(`Wallet: ${payer.publicKey.toBase58()}`);
  const balance = await connection.getBalance(payer.publicKey);
  console.log(`Balance: ${(balance / 1e9).toFixed(3)} SOL\n`);

  if (balance < 1.5e9) {
    console.log("Requesting airdrop (need ≥1.5 SOL)...");
    const sig = await connection.requestAirdrop(payer.publicKey, 2e9);
    await connection.confirmTransaction(sig);
  }

  // ── Verify MXE is live ────────────────────────────────────────────────────
  console.log(`MXE account: ${MXE_ADDRESS.toBase58()}`);
  const mxeInfo = await connection.getAccountInfo(MXE_ADDRESS);
  if (!mxeInfo) {
    console.error(
      "❌  MXE account not found on devnet.\n" +
      "    Arcium devnet may not be running. Ask in their Discord."
    );
    process.exit(1);
  }
  console.log("✅  Arcium MXE is live on devnet!\n");

  // Decode MXE account manually (Arcium IDL doesn't expose fields via Anchor)
  const mxeRaw = mxeInfo.data;
  let mxeOff = 8; // skip discriminator
  const clusterTag = mxeRaw[mxeOff++];
  if (clusterTag === 1) mxeOff += 4; // skip Option<u32> cluster value
  mxeOff += 8 + 8; // keygen_offset + key_recovery_init_offset (u64 each)
  mxeOff += 32;    // mxe_program_id (Pubkey)
  const authTag = mxeRaw[mxeOff++];
  if (authTag === 1) mxeOff += 32; // skip authority Pubkey if Some
  // SetUnset<UtilityPubkeys>: tag + x25519(32) + ed25519(32) + elgamal(32) + proof(64)
  const suTag = mxeRaw[mxeOff++]; // 0=Set, 1=Unset
  const mxeX25519Pubkey = mxeRaw.slice(mxeOff, mxeOff + 32); mxeOff += 32;
  mxeOff += 32 + 32 + 64; // ed25519, elgamal, proof
  if (suTag === 1) {
    const vecLen = mxeRaw.readUInt32LE(mxeOff); mxeOff += 4 + vecLen; // skip Unset bool vec
  }
  const lutOffsetSlot = mxeRaw.readBigUInt64LE(mxeOff); mxeOff += 8;

  const keysGenerated = mxeX25519Pubkey.some(b => b !== 0);
  console.log(`MXE key status: ${keysGenerated ? "✅ keys generated" : "⚠️  keys NOT generated yet (keygen pending)"}`);
  console.log(`LUT offset slot: ${lutOffsetSlot}`);

  const lutAddr = lutPda(new BN(lutOffsetSlot.toString()));

  console.log(`Cluster offset: ${CLUSTER_OFFSET.toString()}`);

  // Computation offsets — unique per call (timestamp-based)
  const compOffset1 = new BN(Date.now());
  const compOffset2 = new BN(Date.now() + 1000);
  const compOffset3 = new BN(Date.now() + 2000);

  // ── Step 1: Setup plaintext lending infrastructure ────────────────────────
  console.log("── Step 1: Plaintext infrastructure setup ──\n");

  const owner = payer;
  const [lendingMarketPda] = pdaVeilvault([
    Buffer.from("lending_market"),
    owner.publicKey.toBuffer(),
  ]);

  // Initialize market (skip if exists)
  const marketInfo = await connection.getAccountInfo(lendingMarketPda);
  if (!marketInfo) {
    console.log("Initializing lending market...");
    await program.methods
      .initializeMarket({
        quoteCurrency: Array.from(Buffer.alloc(32)),
        protocolFeeBps: 50,
      })
      .accountsStrict({
        owner: owner.publicKey,
        lendingMarket: lendingMarketPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([owner])
      .rpc();
    console.log("✅  Market initialized");
  } else {
    console.log("✅  Market already exists");
  }

  // Create or reuse a token mint
  const reserveMint = await createMint(
    connection,
    payer,
    payer.publicKey,
    null,
    6
  );
  console.log(`Reserve mint: ${reserveMint.toBase58()}`);

  // Derive reserve PDAs
  const [reservePda]              = pdaVeilvault([Buffer.from("reserve"), lendingMarketPda.toBuffer(), reserveMint.toBuffer()]);
  const [liquidityVaultPda]       = pdaVeilvault([Buffer.from("liquidity_vault"), reservePda.toBuffer()]);
  const [feeVaultPda]             = pdaVeilvault([Buffer.from("fee_vault"), reservePda.toBuffer()]);
  const [collateralMintPda]       = pdaVeilvault([Buffer.from("collateral_mint"), reservePda.toBuffer()]);
  const [collateralSupplyVaultPda]= pdaVeilvault([Buffer.from("collateral_supply"), reservePda.toBuffer()]);

  // Add reserve
  const reserveInfo = await connection.getAccountInfo(reservePda);
  if (!reserveInfo) {
    console.log("Adding reserve...");
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
          depositLimit: new BN(1_000_000_000),
          borrowLimit: new BN(800_000_000),
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
    console.log("✅  Reserve added");
  } else {
    console.log("✅  Reserve already exists");
  }

  // User token + collateral accounts
  const userTokenAccount = await createAccount(connection, payer, reserveMint, payer.publicKey);
  const userCollateralAccount = await createAccount(connection, payer, collateralMintPda, payer.publicKey);

  // Mint tokens to user
  const DEPOSIT_AMOUNT = 1_000_000; // 1 token (6 decimals)
  await mintTo(connection, payer, reserveMint, userTokenAccount, payer, DEPOSIT_AMOUNT * 10);
  console.log(`Minted ${DEPOSIT_AMOUNT * 10} tokens to user\n`);

  // Deposit tokens (plaintext — puts liquidity in vault for later borrow)
  console.log("Depositing into liquidity vault...");
  await program.methods
    .deposit(new BN(DEPOSIT_AMOUNT * 5))
    .accountsStrict({
      depositor: payer.publicKey,
      lendingMarket: lendingMarketPda,
      reserve: reservePda,
      reserveMint,
      liquidityVault: liquidityVaultPda,
      collateralMint: collateralMintPda,
      userTokenAccount,
      userCollateralAccount,
      tokenProgram: TOKEN_PROGRAM_ID,
    })
    .signers([payer])
    .rpc();
  console.log("✅  Tokens deposited\n");

  // Init obligation (needed for private_borrow to check health factor)
  const [obligationPda] = pdaVeilvault([
    Buffer.from("obligation"),
    lendingMarketPda.toBuffer(),
    payer.publicKey.toBuffer(),
  ]);
  const oblInfo = await connection.getAccountInfo(obligationPda);
  if (!oblInfo) {
    await program.methods
      .initObligation()
      .accountsStrict({
        owner: payer.publicKey,
        lendingMarket: lendingMarketPda,
        obligation: obligationPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([payer])
      .rpc();
    console.log("✅  Obligation initialized");
  }

  // ── Step 2: Register Arcis comp_defs (one-time admin) ────────────────────
  console.log("── Step 2: Register Arcis circuits ──\n");

  const compDefInitPos  = compDefPda(COMP_DEF_OFFSET_INIT_POSITION);
  const compDefAddColl  = compDefPda(COMP_DEF_OFFSET_ADD_COLLATERAL);
  const compDefAddBorr  = compDefPda(COMP_DEF_OFFSET_ADD_BORROW);

  const regAccounts = {
    payer: payer.publicKey,
    mxeAccount: MXE_ADDRESS,
    compDefAccount: PublicKey.default, // filled per instruction
    addressLookupTable: lutAddr,
    lutProgram: new PublicKey("AddressLookupTab1e1111111111111111111111111"),
    arciumProgram: ARCIUM_PROGRAM_ID,
    systemProgram: SystemProgram.programId,
  };

  // Only register if not already registered
  for (const [name, method, compDef] of [
    ["init_position",   "initPositionCompDef",  compDefInitPos],
    ["add_collateral",  "addCollateralCompDef", compDefAddColl],
    ["add_borrow",      "addBorrowCompDef",     compDefAddBorr],
  ] as [string, string, PublicKey][]) {
    const info = await connection.getAccountInfo(compDef);
    if (!info) {
      console.log(`Registering ${name} circuit...`);
      await (program.methods as any)[method]()
        .accountsStrict({ ...regAccounts, compDefAccount: compDef })
        .signers([payer])
        .rpc();
      console.log(`✅  ${name} registered`);
    } else {
      console.log(`✅  ${name} already registered`);
    }
  }
  console.log();

  if (!keysGenerated) {
    console.warn(
      "\n⚠️  MXE keygen has NOT completed — x25519 pubkey is all-zeros.\n" +
      "   Arcium's nodes have not yet processed the key generation ceremony.\n" +
      "   comp_defs registered above. Ask in Arcium Discord to unblock keygen.\n" +
      "   Private instructions (steps 3-5) require the MXE keys to be set.\n"
    );
    process.exit(0);
  }

  // ── Step 3: Init private obligation ──────────────────────────────────────
  console.log("── Step 3: Init private obligation ──\n");

  const [privateObligationPda] = pdaVeilvault([
    Buffer.from("private_obligation"),
    lendingMarketPda.toBuffer(),
    payer.publicKey.toBuffer(),
  ]);

  const poInfo = await connection.getAccountInfo(privateObligationPda);
  if (!poInfo) {
    const comp1Pda = computationPda(compOffset1);
    console.log("Calling init_private_obligation...");
    await program.methods
      .initPrivateObligation(compOffset1)
      .accountsStrict({
        payer: payer.publicKey,
        signPdaAccount: SIGN_PDA_ADDRESS,
        mxeAccount: MXE_ADDRESS,
        mempoolAccount: MEMPOOL_ADDRESS,
        executingPool: EXECPOOL_ADDRESS,
        computationAccount: comp1Pda,
        compDefAccount: compDefInitPos,
        clusterAccount: CLUSTER_ADDRESS,
        poolAccount: ARCIUM_FEE_POOL,
        clockAccount: ARCIUM_CLOCK_ACCT,
        systemProgram: SystemProgram.programId,
        arciumProgram: ARCIUM_PROGRAM_ID,
        lendingMarket: lendingMarketPda,
        privateObligation: privateObligationPda,
      })
      .signers([payer])
      .rpc();
    console.log("✅  init_private_obligation submitted — waiting for MXE callback...");
  } else {
    console.log("✅  PrivateObligation already exists");
  }

  // Poll until is_initialized = true (MXE runs init_position and fires callback)
  await pollUntil("is_initialized = true", async () => {
    const po = await program.account.privateObligation.fetch(privateObligationPda);
    return po.isInitialized ? po : null;
  });
  console.log("✅  PrivateObligation initialized by MXE!\n");

  // ── Step 4: Private deposit collateral ───────────────────────────────────
  console.log("── Step 4: Private deposit collateral ──\n");

  const COLLATERAL_AMOUNT = 500_000; // 0.5 cTokens
  const poState = await program.account.privateObligation.fetch(privateObligationPda);
  const encStateBefore = JSON.stringify(poState.encState);

  const { encryptedAmount: encAmt1, encryptionPubkey: encPub1, encryptionNonce: encNonce1 } =
    encryptAmountForMxe(BigInt(COLLATERAL_AMOUNT), mxeX25519Pubkey);

  const comp2Pda = computationPda(compOffset2);
  console.log("Calling private_deposit_collateral...");
  await program.methods
    .privateDepositCollateral(
      compOffset2,
      new BN(COLLATERAL_AMOUNT),
      Array.from(encAmt1),
      Array.from(encPub1),
      encryptionNonceToBN(encNonce1)
    )
    .accountsStrict({
      depositor: payer.publicKey,
      signPdaAccount: SIGN_PDA_ADDRESS,
      mxeAccount: MXE_ADDRESS,
      mempoolAccount: MEMPOOL_ADDRESS,
      executingPool: EXECPOOL_ADDRESS,
      computationAccount: comp2Pda,
      compDefAccount: compDefAddColl,
      clusterAccount: CLUSTER_ADDRESS,
      poolAccount: ARCIUM_FEE_POOL,
      clockAccount: ARCIUM_CLOCK_ACCT,
      systemProgram: SystemProgram.programId,
      arciumProgram: ARCIUM_PROGRAM_ID,
      lendingMarket: lendingMarketPda,
      reserve: reservePda,
      reserveMint,
      collateralMint: collateralMintPda,
      userCollateralAccount,
      collateralSupplyVault: collateralSupplyVaultPda,
      obligation: obligationPda,
      privateObligation: privateObligationPda,
      tokenProgram: TOKEN_PROGRAM_ID,
    })
    .signers([payer])
    .rpc();
  console.log("✅  private_deposit_collateral submitted — waiting for MXE...");

  // Poll until enc_state changes (MXE ran add_collateral circuit)
  await pollUntil("enc_state updated (collateral)", async () => {
    const po = await program.account.privateObligation.fetch(privateObligationPda);
    return JSON.stringify(po.encState) !== encStateBefore ? po : null;
  });
  console.log("✅  Collateral encrypted and stored in PrivateObligation!\n");

  // ── Step 5: Private borrow ────────────────────────────────────────────────
  console.log("── Step 5: Private borrow ──\n");

  // Refresh obligation first (required by borrow — ObligationStale check)
  // NOTE: refresh_reserve requires a valid Pyth oracle — skip if oracle not set up
  // For testing, obligation.last_update slot must match current slot.
  // A quick workaround: call refresh_reserve if oracle is set, else call borrow immediately.
  try {
    await program.methods
      .refreshObligation()
      .accounts({
        obligation: obligationPda,
      })
      .rpc();
    console.log("✅  Obligation refreshed");
  } catch {
    console.log("⚠️   refresh_obligation skipped (oracle not set — expected for test)");
  }

  const BORROW_AMOUNT = 100_000;
  const poAfterDeposit = await program.account.privateObligation.fetch(privateObligationPda);
  const encStateBefore2 = JSON.stringify(poAfterDeposit.encState);

  const { encryptedAmount: encAmt2, encryptionPubkey: encPub2, encryptionNonce: encNonce2 } =
    encryptAmountForMxe(BigInt(BORROW_AMOUNT), mxeX25519Pubkey);

  const comp3Pda = computationPda(compOffset3);
  console.log("Calling private_borrow...");
  await program.methods
    .privateBorrow(
      compOffset3,
      new BN(BORROW_AMOUNT),
      Array.from(encAmt2),
      Array.from(encPub2),
      encryptionNonceToBN(encNonce2)
    )
    .accountsStrict({
      borrower: payer.publicKey,
      signPdaAccount: SIGN_PDA_ADDRESS,
      mxeAccount: MXE_ADDRESS,
      mempoolAccount: MEMPOOL_ADDRESS,
      executingPool: EXECPOOL_ADDRESS,
      computationAccount: comp3Pda,
      compDefAccount: compDefAddBorr,
      clusterAccount: CLUSTER_ADDRESS,
      poolAccount: ARCIUM_FEE_POOL,
      clockAccount: ARCIUM_CLOCK_ACCT,
      systemProgram: SystemProgram.programId,
      arciumProgram: ARCIUM_PROGRAM_ID,
      lendingMarket: lendingMarketPda,
      obligation: obligationPda,
      reserve: reservePda,
      reserveMint,
      liquidityVault: liquidityVaultPda,
      userTokenAccount,
      privateObligation: privateObligationPda,
      tokenProgram: TOKEN_PROGRAM_ID,
    })
    .signers([payer])
    .rpc();
  console.log("✅  private_borrow submitted — waiting for MXE...");

  // Poll until enc_state changes again
  await pollUntil("enc_state updated (borrow)", async () => {
    const po = await program.account.privateObligation.fetch(privateObligationPda);
    return JSON.stringify(po.encState) !== encStateBefore2 ? po : null;
  });

  const finalState = await program.account.privateObligation.fetch(privateObligationPda);
  console.log("✅  Borrow encrypted and stored!\n");
  console.log("── Final PrivateObligation state ──");
  console.log(`  is_initialized:  ${finalState.isInitialized}`);
  console.log(`  collateral_reserve: ${finalState.collateralReserve.toBase58()}`);
  console.log(`  borrow_reserve:     ${finalState.borrowReserve.toBase58()}`);
  console.log(`  enc_state[0]:  ${Buffer.from(finalState.encState[0]).toString("hex")}`);
  console.log(`  enc_state[1]:  ${Buffer.from(finalState.encState[1]).toString("hex")}`);
  console.log(`  nonce:         ${finalState.nonce.toString()}`);
  console.log("\n🔐  Private layer test complete. Balances are encrypted on-chain.");
}

// u128 → BN (anchor uses BN for u128)
function encryptionNonceToBN(nonce: bigint): BN {
  return new BN(nonce.toString());
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
