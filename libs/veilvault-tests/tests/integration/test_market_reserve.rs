use super::setup::*;

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
    // Trying to init the same market again should fail (account already exists).
    let result = try_send(
        &mut env.svm,
        &[ix_initialize_market(env.owner.pubkey(), env.lending_market, [0u8; 32], 50)],
        &[&env.owner],
    );
    assert!(result.is_err(), "duplicate initialize_market should fail");
}
