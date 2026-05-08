use litesvm::LiteSVM;
use sha2::{Digest, Sha256};
use solana_program::program_pack::Pack;
use solana_sdk::{
    clock::Clock,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};
use spl_token::state::Mint;
use std::path::PathBuf;

// ── Constants ────────────────────────────────────────────────────────────────

pub fn veilvault_id() -> Pubkey {
    "CMbnY6XXekgVZvFHwmB6yC15TD5x7anD1XmHrm218Wbs"
        .parse()
        .unwrap()
}

/// Starting unix_timestamp we pin the LiteSVM clock to.
pub const BASE_TIMESTAMP: i64 = 1_700_000_000;

// ── TestEnv ──────────────────────────────────────────────────────────────────

pub struct TestEnv {
    pub svm: LiteSVM,
    pub owner: Keypair,
    /// Pubkey of the LendingMarket PDA.
    pub lending_market: Pubkey,
    /// SPL token mint used as the reserve's underlying asset.
    pub reserve_mint: Pubkey,
    /// Reserve PDA.
    pub reserve: Pubkey,
    /// Liquidity vault PDA.
    pub liquidity_vault: Pubkey,
    /// Fee vault PDA.
    pub fee_vault: Pubkey,
    /// cToken mint PDA.
    pub collateral_mint: Pubkey,
    /// Collateral supply vault PDA (holds borrowers' locked cTokens).
    pub collateral_supply_vault: Pubkey,
    /// Mock Pyth PriceUpdateV2 account for this reserve.
    pub pyth_oracle: Pubkey,
}

/// Spin up a fully-initialised environment: market + one USDC-like reserve.
/// The pyth oracle is pre-loaded with price $1.00.
///
/// Prerequisite: `cargo build-sbf` must have been run from `veilvault/` so
/// that `target/deploy/veilvault.so` exists.
pub fn setup_env() -> TestEnv {
    let mut svm = LiteSVM::new();

    // load compiled program
    let so_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/deploy/veilvault.so");
    let program_bytes = std::fs::read(&so_path).unwrap_or_else(|e| {
        panic!(
            "veilvault.so not found — run `cargo build-sbf` first.\nPath: {}\nError: {}",
            so_path.display(),
            e
        )
    });
    svm.add_program(veilvault_id(), &program_bytes);

    // pin clock so Pyth staleness checks pass
    let mut clock: Clock = svm.get_sysvar();
    clock.slot = 1;
    clock.unix_timestamp = BASE_TIMESTAMP;
    svm.set_sysvar(&clock);

    // fund owner
    let owner = Keypair::new();
    svm.airdrop(&owner.pubkey(), 100_000_000_000).unwrap();

    // create underlying token mint (6 decimals, like USDC)
    let mint_kp = Keypair::new();
    create_spl_mint(&mut svm, &owner, &mint_kp, 6);
    let reserve_mint = mint_kp.pubkey();

    // derive all PDAs
    let lending_market = find_lending_market(owner.pubkey()).0;
    let reserve = find_reserve(lending_market, reserve_mint).0;
    let liquidity_vault = find_liquidity_vault(reserve).0;
    let fee_vault = find_fee_vault(reserve).0;
    let collateral_mint = find_collateral_mint(reserve).0;
    let collateral_supply_vault = find_collateral_supply(reserve).0;

    // inject mock Pyth oracle priced at $1.00
    let pyth_oracle = super::pyth::create_price_account(&mut svm, 1.0, BASE_TIMESTAMP);

    // initialize_market
    send(
        &mut svm,
        &[ix_initialize_market(
            owner.pubkey(),
            lending_market,
            [0u8; 32],
            50,
        )],
        &[&owner],
    );

    // add_reserve with sensible defaults
    send(
        &mut svm,
        &[ix_add_reserve(
            owner.pubkey(),
            lending_market,
            reserve,
            reserve_mint,
            liquidity_vault,
            fee_vault,
            collateral_mint,
            collateral_supply_vault,
            ReserveConfigArgs {
                status: 0,
                min_borrow_rate_bps: 200,
                optimal_borrow_rate_bps: 2_000,
                max_borrow_rate_bps: 10_000,
                optimal_utilization_bps: 8_000,
                loan_to_value_pct: 75,
                liquidation_threshold_pct: 80,
                liquidation_bonus_pct: 500,
                deposit_limit: u64::MAX / 2,
                borrow_limit: u64::MAX / 2,
                protocol_fee: 50,
                pyth_oracle,
            },
        )],
        &[&owner],
    );

    TestEnv {
        svm,
        owner,
        lending_market,
        reserve_mint,
        reserve,
        liquidity_vault,
        fee_vault,
        collateral_mint,
        collateral_supply_vault,
        pyth_oracle,
    }
}

// ── Clock helpers ─────────────────────────────────────────────────────────────

pub fn advance_slots(svm: &mut LiteSVM, slots: u64) {
    let mut clock: Clock = svm.get_sysvar();
    clock.slot += slots;
    clock.unix_timestamp += slots as i64;
    svm.set_sysvar(&clock);
    svm.expire_blockhash(); // rotate hash so identical ixs get a new tx signature
}

pub fn current_slot(svm: &LiteSVM) -> u64 {
    let clock: Clock = svm.get_sysvar();
    clock.slot
}

// ── Token helpers ─────────────────────────────────────────────────────────────

pub fn create_spl_mint(svm: &mut LiteSVM, payer: &Keypair, mint_kp: &Keypair, decimals: u8) {
    let rent = solana_sdk::rent::Rent::default().minimum_balance(Mint::LEN);
    let ixs = [
        system_instruction::create_account(
            &payer.pubkey(),
            &mint_kp.pubkey(),
            rent,
            Mint::LEN as u64,
            &spl_token::ID,
        ),
        spl_token::instruction::initialize_mint(
            &spl_token::ID,
            &mint_kp.pubkey(),
            &payer.pubkey(),
            None,
            decimals,
        )
        .unwrap(),
    ];
    send(svm, &ixs, &[payer, mint_kp]);
}

/// Create a plain SPL token account owned by `owner`.
pub fn create_token_account(
    svm: &mut LiteSVM,
    payer: &Keypair,
    mint: Pubkey,
    owner: Pubkey,
) -> Pubkey {
    let ta_kp = Keypair::new();
    let rent = solana_sdk::rent::Rent::default().minimum_balance(spl_token::state::Account::LEN);
    let ixs = [
        system_instruction::create_account(
            &payer.pubkey(),
            &ta_kp.pubkey(),
            rent,
            spl_token::state::Account::LEN as u64,
            &spl_token::ID,
        ),
        spl_token::instruction::initialize_account(&spl_token::ID, &ta_kp.pubkey(), &mint, &owner)
            .unwrap(),
    ];
    send(svm, &ixs, &[payer, &ta_kp]);
    ta_kp.pubkey()
}

/// Mint `amount` tokens to `dest` (payer is the mint authority).
pub fn mint_tokens(
    svm: &mut LiteSVM,
    authority: &Keypair,
    mint: Pubkey,
    dest: Pubkey,
    amount: u64,
) {
    let ix = spl_token::instruction::mint_to(
        &spl_token::ID,
        &mint,
        &dest,
        &authority.pubkey(),
        &[],
        amount,
    )
    .unwrap();
    send(svm, &[ix], &[authority]);
}

/// Read the token balance from a spl-token account (raw u64 amount field).
pub fn token_balance(svm: &LiteSVM, account: Pubkey) -> u64 {
    let data = svm
        .get_account(&account)
        .expect("token account not found")
        .data;
    // SPL token Account layout: [0..32] mint, [32..64] owner, [64..72] amount
    u64::from_le_bytes(data[64..72].try_into().unwrap())
}

// ── Transaction helper ────────────────────────────────────────────────────────

pub fn send(svm: &mut LiteSVM, ixs: &[Instruction], signers: &[&Keypair]) {
    let blockhash = svm.latest_blockhash();
    let tx =
        Transaction::new_signed_with_payer(ixs, Some(&signers[0].pubkey()), signers, blockhash);
    svm.send_transaction(tx)
        .unwrap_or_else(|e| panic!("transaction failed: {e:?}"));
}

/// Try sending and return the error string instead of panicking.
pub fn try_send(
    svm: &mut LiteSVM,
    ixs: &[Instruction],
    signers: &[&Keypair],
) -> Result<(), String> {
    let blockhash = svm.latest_blockhash();
    let tx =
        Transaction::new_signed_with_payer(ixs, Some(&signers[0].pubkey()), signers, blockhash);
    svm.send_transaction(tx)
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

// ── PDA derivation ────────────────────────────────────────────────────────────

pub fn find_lending_market(owner: Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"lending_market", owner.as_ref()], &veilvault_id())
}
pub fn find_reserve(market: Pubkey, mint: Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"reserve", market.as_ref(), mint.as_ref()],
        &veilvault_id(),
    )
}
pub fn find_liquidity_vault(reserve: Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"liquidity_vault", reserve.as_ref()], &veilvault_id())
}
pub fn find_fee_vault(reserve: Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"fee_vault", reserve.as_ref()], &veilvault_id())
}
pub fn find_collateral_mint(reserve: Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"collateral_mint", reserve.as_ref()], &veilvault_id())
}
pub fn find_collateral_supply(reserve: Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"collateral_supply", reserve.as_ref()], &veilvault_id())
}
pub fn find_obligation(market: Pubkey, owner: Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"obligation", market.as_ref(), owner.as_ref()],
        &veilvault_id(),
    )
}

// ── Discriminator helper ──────────────────────────────────────────────────────

fn disc(method: &str) -> [u8; 8] {
    let mut h = Sha256::new();
    h.update(format!("global:{method}").as_bytes());
    h.finalize()[..8].try_into().unwrap()
}

// ── Instruction builders ──────────────────────────────────────────────────────

pub fn ix_initialize_market(
    owner: Pubkey,
    lending_market: Pubkey,
    quote_currency: [u8; 32],
    protocol_fee_bps: u16,
) -> Instruction {
    let mut data = disc("initialize_market").to_vec();
    data.extend_from_slice(&quote_currency);
    data.extend_from_slice(&protocol_fee_bps.to_le_bytes());
    Instruction {
        program_id: veilvault_id(),
        accounts: vec![
            AccountMeta::new(owner, true),
            AccountMeta::new(lending_market, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data,
    }
}

pub struct ReserveConfigArgs {
    pub status: u8,
    pub min_borrow_rate_bps: u16,
    pub optimal_borrow_rate_bps: u16,
    pub max_borrow_rate_bps: u16,
    pub optimal_utilization_bps: u16,
    pub loan_to_value_pct: u8,
    pub liquidation_threshold_pct: u8,
    pub liquidation_bonus_pct: u16,
    pub deposit_limit: u64,
    pub borrow_limit: u64,
    pub protocol_fee: u16,
    pub pyth_oracle: Pubkey,
}

pub fn ix_add_reserve(
    owner: Pubkey,
    lending_market: Pubkey,
    reserve: Pubkey,
    reserve_mint: Pubkey,
    liquidity_vault: Pubkey,
    fee_vault: Pubkey,
    collateral_mint: Pubkey,
    collateral_supply_vault: Pubkey,
    cfg: ReserveConfigArgs,
) -> Instruction {
    let mut data = disc("add_reserve").to_vec();
    // Borsh for AddReserveArgs { config: InitReserveConfigParams }
    data.push(cfg.status);
    data.extend_from_slice(&cfg.min_borrow_rate_bps.to_le_bytes());
    data.extend_from_slice(&cfg.optimal_borrow_rate_bps.to_le_bytes());
    data.extend_from_slice(&cfg.max_borrow_rate_bps.to_le_bytes());
    data.extend_from_slice(&cfg.optimal_utilization_bps.to_le_bytes());
    data.push(cfg.loan_to_value_pct);
    data.push(cfg.liquidation_threshold_pct);
    data.extend_from_slice(&cfg.liquidation_bonus_pct.to_le_bytes());
    data.extend_from_slice(&cfg.deposit_limit.to_le_bytes());
    data.extend_from_slice(&cfg.borrow_limit.to_le_bytes());
    data.extend_from_slice(&cfg.protocol_fee.to_le_bytes());
    data.extend_from_slice(cfg.pyth_oracle.as_ref());
    Instruction {
        program_id: veilvault_id(),
        accounts: vec![
            AccountMeta::new(owner, true),
            AccountMeta::new(lending_market, false),
            AccountMeta::new(reserve, false),
            AccountMeta::new_readonly(reserve_mint, false),
            AccountMeta::new(liquidity_vault, false),
            AccountMeta::new(fee_vault, false),
            AccountMeta::new(collateral_mint, false),
            AccountMeta::new(collateral_supply_vault, false),
            AccountMeta::new_readonly(spl_token::ID, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data,
    }
}

pub fn ix_init_obligation(
    owner: Pubkey,
    lending_market: Pubkey,
    obligation: Pubkey,
) -> Instruction {
    Instruction {
        program_id: veilvault_id(),
        accounts: vec![
            AccountMeta::new(owner, true),
            AccountMeta::new_readonly(lending_market, false),
            AccountMeta::new(obligation, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data: disc("init_obligation").to_vec(),
    }
}

pub fn ix_deposit(
    depositor: Pubkey,
    lending_market: Pubkey,
    reserve: Pubkey,
    reserve_mint: Pubkey,
    liquidity_vault: Pubkey,
    collateral_mint: Pubkey,
    user_token_account: Pubkey,
    user_collateral_account: Pubkey,
    amount: u64,
) -> Instruction {
    let mut data = disc("deposit").to_vec();
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: veilvault_id(),
        accounts: vec![
            AccountMeta::new(depositor, true),
            AccountMeta::new_readonly(lending_market, false),
            AccountMeta::new(reserve, false),
            AccountMeta::new_readonly(reserve_mint, false),
            AccountMeta::new(liquidity_vault, false),
            AccountMeta::new(collateral_mint, false),
            AccountMeta::new(user_token_account, false),
            AccountMeta::new(user_collateral_account, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data,
    }
}

pub fn ix_withdraw(
    depositor: Pubkey,
    lending_market: Pubkey,
    reserve: Pubkey,
    reserve_mint: Pubkey,
    liquidity_vault: Pubkey,
    collateral_mint: Pubkey,
    user_collateral_account: Pubkey,
    user_token_account: Pubkey,
    collateral_amount: u64,
) -> Instruction {
    let mut data = disc("withdraw").to_vec();
    data.extend_from_slice(&collateral_amount.to_le_bytes());
    Instruction {
        program_id: veilvault_id(),
        accounts: vec![
            AccountMeta::new(depositor, true),
            AccountMeta::new_readonly(lending_market, false),
            AccountMeta::new(reserve, false),
            AccountMeta::new_readonly(reserve_mint, false),
            AccountMeta::new(liquidity_vault, false),
            AccountMeta::new(collateral_mint, false),
            AccountMeta::new(user_collateral_account, false),
            AccountMeta::new(user_token_account, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data,
    }
}

pub fn ix_borrow(
    borrower: Pubkey,
    lending_market: Pubkey,
    obligation: Pubkey,
    reserve: Pubkey,
    reserve_mint: Pubkey,
    liquidity_vault: Pubkey,
    user_token_account: Pubkey,
    amount: u64,
) -> Instruction {
    let mut data = disc("borrow").to_vec();
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: veilvault_id(),
        accounts: vec![
            AccountMeta::new(borrower, true),
            AccountMeta::new_readonly(lending_market, false),
            AccountMeta::new(obligation, false),
            AccountMeta::new(reserve, false),
            AccountMeta::new_readonly(reserve_mint, false),
            AccountMeta::new(liquidity_vault, false),
            AccountMeta::new(user_token_account, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data,
    }
}

pub fn ix_repay(
    borrower: Pubkey,
    lending_market: Pubkey,
    obligation: Pubkey,
    reserve: Pubkey,
    reserve_mint: Pubkey,
    liquidity_vault: Pubkey,
    user_token_account: Pubkey,
    amount: u64,
) -> Instruction {
    let mut data = disc("repay").to_vec();
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: veilvault_id(),
        accounts: vec![
            AccountMeta::new(borrower, true),
            AccountMeta::new_readonly(lending_market, false),
            AccountMeta::new(obligation, false),
            AccountMeta::new(reserve, false),
            AccountMeta::new_readonly(reserve_mint, false),
            AccountMeta::new(liquidity_vault, false),
            AccountMeta::new(user_token_account, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data,
    }
}

pub fn ix_refresh_reserve(
    lending_market: Pubkey,
    reserve: Pubkey,
    pyth_oracle: Pubkey,
) -> Instruction {
    Instruction {
        program_id: veilvault_id(),
        accounts: vec![
            AccountMeta::new_readonly(lending_market, false),
            AccountMeta::new(reserve, false),
            AccountMeta::new_readonly(pyth_oracle, false),
        ],
        data: disc("refresh_reserve").to_vec(),
    }
}

/// `reserve_accounts` are passed as `remaining_accounts` in the program.
pub fn ix_refresh_obligation(
    lending_market: Pubkey,
    obligation: Pubkey,
    reserve_accounts: &[Pubkey],
) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new_readonly(lending_market, false),
        AccountMeta::new(obligation, false),
    ];
    for r in reserve_accounts {
        accounts.push(AccountMeta::new_readonly(*r, false));
    }
    Instruction {
        program_id: veilvault_id(),
        accounts,
        data: disc("refresh_obligation").to_vec(),
    }
}

pub fn ix_deposit_collateral(
    depositor: Pubkey,
    lending_market: Pubkey,
    reserve: Pubkey,
    reserve_mint: Pubkey,
    collateral_mint: Pubkey,
    user_collateral_account: Pubkey,
    collateral_supply_vault: Pubkey,
    obligation: Pubkey,
    collateral_amount: u64,
) -> Instruction {
    let mut data = disc("deposit_collateral").to_vec();
    data.extend_from_slice(&collateral_amount.to_le_bytes());
    Instruction {
        program_id: veilvault_id(),
        accounts: vec![
            AccountMeta::new(depositor, true),
            AccountMeta::new_readonly(lending_market, false),
            AccountMeta::new_readonly(reserve, false),
            AccountMeta::new_readonly(reserve_mint, false),
            AccountMeta::new(collateral_mint, false),
            AccountMeta::new(user_collateral_account, false),
            AccountMeta::new(collateral_supply_vault, false),
            AccountMeta::new(obligation, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data,
    }
}

pub fn ix_withdraw_collateral(
    depositor: Pubkey,
    lending_market: Pubkey,
    reserve: Pubkey,
    reserve_mint: Pubkey,
    collateral_mint: Pubkey,
    collateral_supply_vault: Pubkey,
    user_collateral_account: Pubkey,
    obligation: Pubkey,
    collateral_amount: u64,
) -> Instruction {
    let mut data = disc("withdraw_collateral").to_vec();
    data.extend_from_slice(&collateral_amount.to_le_bytes());
    Instruction {
        program_id: veilvault_id(),
        accounts: vec![
            AccountMeta::new(depositor, true),
            AccountMeta::new_readonly(lending_market, false),
            AccountMeta::new_readonly(reserve, false),
            AccountMeta::new_readonly(reserve_mint, false),
            AccountMeta::new(collateral_mint, false),
            AccountMeta::new(collateral_supply_vault, false),
            AccountMeta::new(user_collateral_account, false),
            AccountMeta::new(obligation, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data,
    }
}

pub fn ix_liquidate(
    liquidator: Pubkey,
    lending_market: Pubkey,
    obligation: Pubkey,
    repay_reserve: Pubkey,
    repay_reserve_mint: Pubkey,
    repay_liquidity_vault: Pubkey,
    liquidator_repay_token_account: Pubkey,
    withdraw_reserve: Pubkey,
    withdraw_collateral_mint: Pubkey,
    collateral_supply_vault: Pubkey,
    liquidator_collateral_account: Pubkey,
    repay_amount: u64,
) -> Instruction {
    let mut data = disc("liquidate").to_vec();
    data.extend_from_slice(&repay_amount.to_le_bytes());
    Instruction {
        program_id: veilvault_id(),
        accounts: vec![
            AccountMeta::new(liquidator, true),
            AccountMeta::new_readonly(lending_market, false),
            AccountMeta::new(obligation, false),
            AccountMeta::new(repay_reserve, false),
            AccountMeta::new_readonly(repay_reserve_mint, false),
            AccountMeta::new(repay_liquidity_vault, false),
            AccountMeta::new(liquidator_repay_token_account, false),
            AccountMeta::new(withdraw_reserve, false),
            AccountMeta::new(withdraw_collateral_mint, false),
            AccountMeta::new(collateral_supply_vault, false),
            AccountMeta::new(liquidator_collateral_account, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data,
    }
}

pub fn ix_update_reserve_config(
    owner: Pubkey,
    lending_market: Pubkey,
    reserve_mint: Pubkey,
    reserve: Pubkey,
    cfg: ReserveConfigArgs,
) -> Instruction {
    let mut data = disc("update_reserve_config").to_vec();
    data.push(cfg.status);
    data.extend_from_slice(&cfg.min_borrow_rate_bps.to_le_bytes());
    data.extend_from_slice(&cfg.optimal_borrow_rate_bps.to_le_bytes());
    data.extend_from_slice(&cfg.max_borrow_rate_bps.to_le_bytes());
    data.extend_from_slice(&cfg.optimal_utilization_bps.to_le_bytes());
    data.push(cfg.loan_to_value_pct);
    data.push(cfg.liquidation_threshold_pct);
    data.extend_from_slice(&cfg.liquidation_bonus_pct.to_le_bytes());
    data.extend_from_slice(&cfg.deposit_limit.to_le_bytes());
    data.extend_from_slice(&cfg.borrow_limit.to_le_bytes());
    data.extend_from_slice(&cfg.protocol_fee.to_le_bytes());
    data.extend_from_slice(cfg.pyth_oracle.as_ref());
    Instruction {
        program_id: veilvault_id(),
        accounts: vec![
            AccountMeta::new_readonly(owner, true),
            AccountMeta::new_readonly(lending_market, false),
            AccountMeta::new_readonly(reserve_mint, false),
            AccountMeta::new(reserve, false),
        ],
        data,
    }
}

pub fn ix_set_pause(owner: Pubkey, lending_market: Pubkey, paused: bool) -> Instruction {
    let mut data = disc("set_pause").to_vec();
    data.push(paused as u8);
    Instruction {
        program_id: veilvault_id(),
        accounts: vec![
            AccountMeta::new_readonly(owner, true),
            AccountMeta::new(lending_market, false),
        ],
        data,
    }
}
