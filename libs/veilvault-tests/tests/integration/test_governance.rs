use super::setup::*;
use solana_sdk::signature::{Keypair, Signer};

const DEPOSIT_AMOUNT: u64 = 5_000_000; // 5 USDC

fn do_refresh_reserve(env: &mut TestEnv) {
    let clock: solana_sdk::clock::Clock = env.svm.get_sysvar();
    super::pyth::update_price(&mut env.svm, env.pyth_oracle, 1.0, clock.unix_timestamp);
    let owner = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    send(
        &mut env.svm,
        &[ix_refresh_reserve(env.lending_market, env.reserve, env.pyth_oracle)],
        &[&owner],
    );
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn test_set_pause_blocks_deposit() {
    let mut env = setup_env();
    let owner = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();

    // pause the market
    send(
        &mut env.svm,
        &[ix_set_pause(owner.pubkey(), env.lending_market, true)],
        &[&owner],
    );

    // try to deposit — must fail
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();
    let token_acct = create_token_account(&mut env.svm, &user, env.reserve_mint, user.pubkey());
    let ctoken_acct = create_token_account(&mut env.svm, &user, env.collateral_mint, user.pubkey());
    let owner_clone = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    mint_tokens(&mut env.svm, &owner_clone, env.reserve_mint, token_acct, DEPOSIT_AMOUNT);

    let result = try_send(
        &mut env.svm,
        &[ix_deposit(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.liquidity_vault, env.collateral_mint, token_acct, ctoken_acct, DEPOSIT_AMOUNT,
        )],
        &[&user],
    );
    assert!(result.is_err(), "deposit should be blocked when market is paused");
}

#[test]
fn test_set_pause_blocks_borrow() {
    let mut env = setup_env();
    let owner = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();

    // seed pool then pause
    let owner_token = create_token_account(&mut env.svm, &owner, env.reserve_mint, owner.pubkey());
    let owner_ctoken = create_token_account(&mut env.svm, &owner, env.collateral_mint, owner.pubkey());
    mint_tokens(&mut env.svm, &owner, env.reserve_mint, owner_token, DEPOSIT_AMOUNT * 4);
    send(
        &mut env.svm,
        &[ix_deposit(
            owner.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.liquidity_vault, env.collateral_mint, owner_token, owner_ctoken, DEPOSIT_AMOUNT * 4,
        )],
        &[&owner],
    );
    do_refresh_reserve(&mut env);

    // set up borrower with collateral before pausing
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();
    let token_acct = create_token_account(&mut env.svm, &user, env.reserve_mint, user.pubkey());
    let ctoken_acct = create_token_account(&mut env.svm, &user, env.collateral_mint, user.pubkey());
    let owner_clone = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    mint_tokens(&mut env.svm, &owner_clone, env.reserve_mint, token_acct, DEPOSIT_AMOUNT);
    let obligation = find_obligation(env.lending_market, user.pubkey()).0;
    send(&mut env.svm, &[ix_init_obligation(user.pubkey(), env.lending_market, obligation)], &[&user]);
    send(
        &mut env.svm,
        &[ix_deposit(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.liquidity_vault, env.collateral_mint, token_acct, ctoken_acct, DEPOSIT_AMOUNT,
        )],
        &[&user],
    );
    send(
        &mut env.svm,
        &[ix_deposit_collateral(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.collateral_mint, ctoken_acct, env.collateral_supply_vault, obligation, DEPOSIT_AMOUNT,
        )],
        &[&user],
    );
    send(
        &mut env.svm,
        &[ix_refresh_obligation(env.lending_market, obligation, &[env.reserve])],
        &[&user],
    );

    // now pause
    let owner_clone2 = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    send(
        &mut env.svm,
        &[ix_set_pause(owner_clone2.pubkey(), env.lending_market, true)],
        &[&owner_clone2],
    );

    // try to borrow — must fail
    let result = try_send(
        &mut env.svm,
        &[ix_borrow(
            user.pubkey(), env.lending_market, obligation, env.reserve,
            env.reserve_mint, env.liquidity_vault, token_acct, 1_000_000,
        )],
        &[&user],
    );
    assert!(result.is_err(), "borrow should be blocked when market is paused");
}

#[test]
fn test_unpause_re_enables_deposit() {
    let mut env = setup_env();
    let owner = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();

    // pause then immediately unpause
    send(
        &mut env.svm,
        &[ix_set_pause(owner.pubkey(), env.lending_market, true)],
        &[&owner],
    );
    let owner2 = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    send(
        &mut env.svm,
        &[ix_set_pause(owner2.pubkey(), env.lending_market, false)],
        &[&owner2],
    );

    // deposit should now succeed
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();
    let token_acct = create_token_account(&mut env.svm, &user, env.reserve_mint, user.pubkey());
    let ctoken_acct = create_token_account(&mut env.svm, &user, env.collateral_mint, user.pubkey());
    let owner_clone = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    mint_tokens(&mut env.svm, &owner_clone, env.reserve_mint, token_acct, DEPOSIT_AMOUNT);

    send(
        &mut env.svm,
        &[ix_deposit(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.liquidity_vault, env.collateral_mint, token_acct, ctoken_acct, DEPOSIT_AMOUNT,
        )],
        &[&user],
    );
    assert_eq!(
        token_balance(&env.svm, env.liquidity_vault),
        DEPOSIT_AMOUNT,
        "deposit should succeed after unpause"
    );
}

#[test]
fn test_non_owner_cannot_pause() {
    let mut env = setup_env();
    let attacker = Keypair::new();
    env.svm.airdrop(&attacker.pubkey(), 10_000_000_000).unwrap();

    // attacker tries to pause using their own key; PDA seeds won't match
    let result = try_send(
        &mut env.svm,
        &[ix_set_pause(attacker.pubkey(), env.lending_market, true)],
        &[&attacker],
    );
    assert!(result.is_err(), "non-owner should not be able to pause the market");

    // market should still be unpaused
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();
    let token_acct = create_token_account(&mut env.svm, &user, env.reserve_mint, user.pubkey());
    let ctoken_acct = create_token_account(&mut env.svm, &user, env.collateral_mint, user.pubkey());
    let owner = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    mint_tokens(&mut env.svm, &owner, env.reserve_mint, token_acct, DEPOSIT_AMOUNT);

    // deposit should still succeed (market not paused)
    send(
        &mut env.svm,
        &[ix_deposit(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.liquidity_vault, env.collateral_mint, token_acct, ctoken_acct, DEPOSIT_AMOUNT,
        )],
        &[&user],
    );
}

#[test]
fn test_set_pause_blocks_withdraw() {
    let mut env = setup_env();

    // deposit first so the user has cTokens to withdraw
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();
    let token_acct = create_token_account(&mut env.svm, &user, env.reserve_mint, user.pubkey());
    let ctoken_acct = create_token_account(&mut env.svm, &user, env.collateral_mint, user.pubkey());
    let owner = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    mint_tokens(&mut env.svm, &owner, env.reserve_mint, token_acct, DEPOSIT_AMOUNT);
    send(
        &mut env.svm,
        &[ix_deposit(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.liquidity_vault, env.collateral_mint, token_acct, ctoken_acct, DEPOSIT_AMOUNT,
        )],
        &[&user],
    );

    // pause the market
    let owner2 = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    send(&mut env.svm, &[ix_set_pause(owner2.pubkey(), env.lending_market, true)], &[&owner2]);

    // withdraw must be blocked
    let result = try_send(
        &mut env.svm,
        &[ix_withdraw(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.liquidity_vault, env.collateral_mint, ctoken_acct, token_acct, DEPOSIT_AMOUNT,
        )],
        &[&user],
    );
    assert!(result.is_err(), "withdraw should be blocked when market is paused");
}

#[test]
fn test_update_reserve_config_makes_reserve_inactive() {
    let mut env = setup_env();

    // set up user and get cTokens BEFORE making the reserve inactive
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();
    let token_acct = create_token_account(&mut env.svm, &user, env.reserve_mint, user.pubkey());
    let ctoken_acct = create_token_account(&mut env.svm, &user, env.collateral_mint, user.pubkey());
    let owner_clone = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    mint_tokens(&mut env.svm, &owner_clone, env.reserve_mint, token_acct, DEPOSIT_AMOUNT);

    send(
        &mut env.svm,
        &[ix_deposit(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.liquidity_vault, env.collateral_mint, token_acct, ctoken_acct, DEPOSIT_AMOUNT,
        )],
        &[&user],
    );
    let obligation = find_obligation(env.lending_market, user.pubkey()).0;
    send(&mut env.svm, &[ix_init_obligation(user.pubkey(), env.lending_market, obligation)], &[&user]);

    // NOW update config: status = 1 → inactive
    let owner = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    send(
        &mut env.svm,
        &[ix_update_reserve_config(
            owner.pubkey(),
            env.lending_market,
            env.reserve_mint,
            env.reserve,
            ReserveConfigArgs {
                status: 1, // inactive
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
        &[&owner],
    );

    // deposit_collateral checks is_active() — must now fail
    let result = try_send(
        &mut env.svm,
        &[ix_deposit_collateral(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.collateral_mint, ctoken_acct, env.collateral_supply_vault, obligation, DEPOSIT_AMOUNT,
        )],
        &[&user],
    );
    assert!(result.is_err(), "deposit_collateral should fail when reserve is inactive");
}
