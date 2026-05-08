use super::setup::*;
use solana_sdk::signature::{Keypair, Signer};

#[test]
fn test_initialize_market_creates_account() {
    let env = setup_env();
    // If the account doesn't exist the setup_env() call above already panicked,
    // so reaching here means the PDA was created successfully.
    let acct = env.svm.get_account(&env.lending_market);
    assert!(acct.is_some(), "lending_market PDA should exist");
    let acct = acct.unwrap();
    // Anchor accounts: first 8 bytes are the discriminator
    assert!(acct.data.len() > 8, "account should hold LendingMarket data");
}

#[test]
fn test_add_reserve_creates_all_pdas() {
    let env = setup_env();

    assert!(env.svm.get_account(&env.reserve).is_some(), "reserve PDA missing");
    assert!(env.svm.get_account(&env.liquidity_vault).is_some(), "liquidity_vault missing");
    assert!(env.svm.get_account(&env.fee_vault).is_some(), "fee_vault missing");
    assert!(env.svm.get_account(&env.collateral_mint).is_some(), "collateral_mint missing");
    assert!(
        env.svm.get_account(&env.collateral_supply_vault).is_some(),
        "collateral_supply_vault missing"
    );
}

#[test]
fn test_duplicate_market_fails() {
    let mut env = setup_env();
    let result = try_send(
        &mut env.svm,
        &[ix_initialize_market(env.owner.pubkey(), env.lending_market, [0u8; 32], 50)],
        &[&env.owner],
    );
    assert!(result.is_err(), "duplicate initialize_market should fail");
}

#[test]
fn test_initialize_market_rejects_high_fee() {
    let mut svm = litesvm::LiteSVM::new();
    let program_bytes = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/deploy/veilvault.so"),
    )
    .expect("veilvault.so not found — run cargo build-sbf first");
    svm.add_program(veilvault_id(), &program_bytes);

    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

    let lending_market = find_lending_market(payer.pubkey()).0;

    // protocol_fee_bps = 9999 > MAX_PROTOCOL_FEE_BPS (1000) — must be rejected
    let result = try_send(
        &mut svm,
        &[ix_initialize_market(payer.pubkey(), lending_market, [0u8; 32], 9999)],
        &[&payer],
    );
    assert!(result.is_err(), "fee > 1000 bps should be rejected");
}

#[test]
fn test_add_reserve_rejects_non_owner() {
    let mut env = setup_env();

    // attacker creates their own mint and tries to add a reserve to the existing market
    let attacker = Keypair::new();
    env.svm.airdrop(&attacker.pubkey(), 10_000_000_000).unwrap();

    let mint_kp = Keypair::new();
    create_spl_mint(&mut env.svm, &attacker, &mint_kp, 6);
    let fake_mint = mint_kp.pubkey();

    let fake_reserve = find_reserve(env.lending_market, fake_mint).0;
    let fake_liq = find_liquidity_vault(fake_reserve).0;
    let fake_fee = find_fee_vault(fake_reserve).0;
    let fake_coll_mint = find_collateral_mint(fake_reserve).0;
    let fake_coll_supply = find_collateral_supply(fake_reserve).0;

    // attacker signs as owner but lending_market PDA is derived from real owner's key
    let result = try_send(
        &mut env.svm,
        &[ix_add_reserve(
            attacker.pubkey(),
            env.lending_market,
            fake_reserve,
            fake_mint,
            fake_liq,
            fake_fee,
            fake_coll_mint,
            fake_coll_supply,
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
                pyth_oracle: env.pyth_oracle,
            },
        )],
        &[&attacker],
    );
    assert!(result.is_err(), "non-owner should not be able to add a reserve");
}

#[test]
fn test_init_obligation_duplicate_fails() {
    let mut env = setup_env();
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();

    let obligation = find_obligation(env.lending_market, user.pubkey()).0;

    send(
        &mut env.svm,
        &[ix_init_obligation(user.pubkey(), env.lending_market, obligation)],
        &[&user],
    );

    let result = try_send(
        &mut env.svm,
        &[ix_init_obligation(user.pubkey(), env.lending_market, obligation)],
        &[&user],
    );
    assert!(result.is_err(), "duplicate init_obligation should fail");
}
