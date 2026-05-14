// // scripts/init-comp-defs.ts
// import * as anchor from "@coral-xyz/anchor";
// import { Program } from "@coral-xyz/anchor";
// import {
//   getCompDefAccOffset,
//   getArciumProgramId,
//   getMXEAccAddress,
//   uploadCircuit,
//   buildFinalizeCompDefTx,
// } from "@arcium-hq/client";
// import * as fs from "fs";
// import { Veilvault } from "../target/types/veilvault";
// import {
//   Connection,
//   Keypair,
//   PublicKey,
//   SystemProgram,
//   SYSVAR_INSTRUCTIONS_PUBKEY,
//   Transaction,
//   sendAndConfirmTransaction,
// } from "@solana/web3.js";

// // Wallet: defaults to ~/.config/solana/id.json
// const WALLET_PATH =
//   process.env.ANCHOR_WALLET ?? `${process.env.HOME}/.config/solana/id.json`;

// const RPC_URL = "https://api.devnet.solana.com";


// async function initCompDef(name: string, method_name: string) {
//   console.log("\n🔐  VeilVault — Private Layer Test\n");

//   // Setup connection + provider
//   const connection = new Connection(RPC_URL, "confirmed");
//   const walletKp = Keypair.fromSecretKey(
//     Uint8Array.from(JSON.parse(fs.readFileSync(WALLET_PATH, "utf-8")))
//   );
//   const wallet = new anchor.Wallet(walletKp);
//   const provider = new anchor.AnchorProvider(connection, wallet, {
//     commitment: "confirmed",
//     preflightCommitment: "confirmed",
//   });
//   anchor.setProvider(provider);

//   const idl = JSON.parse(
//     fs.readFileSync("target/idl/veilvault.json", "utf-8")
//   );
//   const program = new Program<Veilvault>(idl, provider);
//   const owner = (provider.wallet as anchor.Wallet).payer;

//   const offset = getCompDefAccOffset(name);
//   const [compDefPda] = PublicKey.findProgramAddressSync(
//     [Buffer.from("ComputationDefinitionAccount"), program.programId.toBuffer(), offset],
//     getArciumProgramId()
//   );

//   // Call your program's init_<name>_comp_def instruction
//   // const sig = await (program.methods as any)
//   // [`init${method_name}CompDef`]()
//   //   .accounts({
//   //     compDefAccount: compDefPda,
//   //     payer: owner.publicKey,
//   //     mxeAccount: getMXEAccAddress(program.programId),
//   //   })
//   //   .rpc({ commitment: "confirmed" });
//   // console.log(`${name} comp def init sig:`, sig);

//   const sig = await initInitPosition2CompDef(program, owner);
//   console.log("Comp def initialized:", sig);

//   const sig1 = await initInitPosition2CompDef(program, owner);
//   console.log("Comp def initialized:", sig1);

//   const sig2 = await initInitPosition2CompDef(program, owner);
//   console.log("Comp def initialized:", sig2);

//   const sig3 = await initInitPosition2CompDef(program, owner);
//   console.log("Comp def initialized:", sig3);

//   const sig4 = await initInitPosition2CompDef(program, owner);
//   console.log("Comp def initialized:", sig4);

//   // await program.methods.initPrivateObligation()
//   // .accounts({ compDefAccount: compDefPda, payer: owner.publicKey, mxeAccount: getMXEAccAddress(program.programId) })
//   // .rpc({ commitment: "confirmed" });

//   // Upload circuit (only needed if storing onchain; skip if using OffChain source)
//   // const rawCircuit = fs.readFileSync(`build/${name}.arcis`);
//   // await uploadCircuit(provider, name, program.programId, rawCircuit, true);

//   const finalizeTx = await buildFinalizeCompDefTx(provider, Buffer.from(offset).readUInt32LE(), program.programId);
//   finalizeTx.recentBlockhash = (await provider.connection.getLatestBlockhash()).blockhash;
//   finalizeTx.feePayer = owner.publicKey;
//   await provider.sendAndConfirm(finalizeTx);
// }

// const pascal = (s: string) => s.replace(/(^|_)(\w)/g, (_, __, c) => c.toUpperCase());

// // initCompDef("init_position_v2").catch(console.error);
// // initCompDef("add_collateral_v2").catch(console.error);
// // initCompDef("remove_collateral_2").catch(console.error);
// // initCompDef("add_borrow_v2").catch(console.error);
// // initCompDef("check_health_v2").catch(console.error);


// async function initInitPosition2CompDef(program: anchor.Program<Veilvault>, owner: anchor.web3.Keypair) {
//   const offset = getCompDefAccOffset("init_position_v2");
//   const sig = await program.methods
//     .initInitPosition2CompDef()
//     .accounts({ payer: owner.publicKey, /* mxe_account, comp_def_account, etc. */ })
//     .signers([owner])
//     .rpc();
//   return sig;
// }

// async function initAddCollateral2CompDef(program: anchor.Program<Veilvault>, owner: anchor.web3.Keypair) {
//   const offset = getCompDefAccOffset("add_collateral_v2");
//   const sig = await program.methods
//     .initAddCollateral2CompDef()
//     .accounts({ payer: owner.publicKey, /* mxe_account, comp_def_account, etc. */ })
//     .signers([owner])
//     .rpc();
//   return sig;
// }

// async function initRemoveCollateral2CompDef(program: anchor.Program<Veilvault>, owner: anchor.web3.Keypair) {
//   const offset = getCompDefAccOffset("remove_collateral_2");
//   const sig = await program.methods
//     .initRemoveCollateral2CompDef()
//     .accounts({ payer: owner.publicKey, /* mxe_account, comp_def_account, etc. */ })
//     .signers([owner])
//     .rpc();
//   return sig;
// }

// async function initAddBorrow2CompDef(program: anchor.Program<Veilvault>, owner: anchor.web3.Keypair) {
//   const offset = getCompDefAccOffset("add_borrow_v2");
//   const sig = await program.methods
//     .initAddBorrow2CompDef()
//     .accounts({ payer: owner.publicKey, /* mxe_account, comp_def_account, etc. */ })
//     .signers([owner])
//     .rpc();
//   return sig;
// }

// async function initCheckHealthCompDef(program: anchor.Program<Veilvault>, owner: anchor.web3.Keypair) {
//   const offset = getCompDefAccOffset("check_health_v2");
//   const sig = await program.methods
//     .initCheckHealthCompDef()
//     .accounts({ payer: owner.publicKey, /* mxe_account, comp_def_account, etc. */ })
//     .signers([owner])
//     .rpc();
//   return sig;
// }
