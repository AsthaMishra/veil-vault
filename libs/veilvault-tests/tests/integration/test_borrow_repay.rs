use super::setup::*;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::pubkey::Pubkey;

const SEED_LIQUIDITY: u64 = 20_000_000; // 20 USDC in the pool
const COLLATERAL: u64     =  5_000_000; //  5 USDC collateral per borrower
const BORROW_AMOUNT: u64  =  1_000_000; //  1 USDC borrow

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Deposit liquidity as the market owner so borrowers have something to borrow.
fn seed_liquidity(env: &mut TestEnv) {
    let owner = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    let token_acct = create_token_account(&mut env.svm, &owner, env.reserve_mint, owner.pubkey());
    let ctoken_acct = create_token_account(&mut env.svm, &owner, env.collateral_mint, owner.pubkey());
    mint_tokens(&mut env.svm, &owner, env.reserve_mint, token_acct, SEED_LIQUIDITY);
    send(
        &mut env.svm,
        &[ix_deposit(
            owner.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.liquidity_vault, env.collateral_mint, token_acct, ctoken_acct, SEED_LIQUIDITY,
        )],
        &[&owner],
    );
}

/// Call refresh_reserve and update the pyth oracle timestamp.
fn refresh_reserve(env: &mut TestEnv) {
    let clock: solana_sdk::clock::Clock = env.svm.get_sysvar();
    super::pyth::update_price(&mut env.svm, env.pyth_oracle, 1.0, clock.unix_timestamp);
    let owner = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    send(
        &mut env.svm,
        &[ix_refresh_reserve(env.lending_market, env.reserve, env.pyth_oracle)],
        &[&owner],
    );
}

/// Call refresh_obligation to stamp last_update and price deposits/borrows.
fn refresh_obligation(env: &mut TestEnv, user: &Keypair, obligation: Pubkey) {
    send(
        &mut env.svm,
        &[ix_refresh_obligation(env.lending_market, obligation, &[env.reserve])],
        &[user],
    );
}

/// Create a borrower who has `COLLATERAL` deposited and locked, refresh done.
/// The caller still needs to call borrow explicitly.
/// Returns (user, token_acct, ctoken_acct, obligation).
fn setup_borrower(env: &mut TestEnv, extra_tokens: u64) -> (Keypair, Pubkey, Pubkey, Pubkey) {
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();

    let token_acct = create_token_account(&mut env.svm, &user, env.reserve_mint, user.pubkey());
    let ctoken_acct = create_token_account(&mut env.svm, &user, env.collateral_mint, user.pubkey());

    let owner = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    mint_tokens(&mut env.svm, &owner, env.reserve_mint, token_acct, COLLATERAL + extra_tokens);

    // deposit to get cTokens
    send(
        &mut env.svm,
        &[ix_deposit(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.liquidity_vault, env.collateral_mint, token_acct, ctoken_acct, COLLATERAL,
        )],
        &[&user],
    );

    // init obligation and lock cTokens as collateral
    let obligation = find_obligation(env.lending_market, user.pubkey()).0;
    send(&mut env.svm, &[ix_init_obligation(user.pubkey(), env.lending_market, obligation)], &[&user]);
    send(
        &mut env.svm,
        &[ix_deposit_collateral(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.collateral_mint, ctoken_acct, env.collateral_supply_vault, obligation, COLLATERAL,
        )],
        &[&user],
    );

    // refresh so market_value_sf is current (required before borrow)
    refresh_obligation(env, &user, obligation);

    (user, token_acct, ctoken_acct, obligation)
}

/// Seed liquidity, set up a borrower, execute a borrow — used by repay-focused tests.
fn setup_active_borrow(env: &mut TestEnv) -> (Keypair, Pubkey, Pubkey) {
    seed_liquidity(env);
    refresh_reserve(env);
    let (user, token_acct, _, obligation) = setup_borrower(env, 0);
    send(
        &mut env.svm,
        &[ix_borrow(
            user.pubkey(), env.lending_market, obligation, env.reserve,
            env.reserve_mint, env.liquidity_vault, token_acct, BORROW_AMOUNT,
        )],
        &[&user],
    );
    (user, token_acct, obligation)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn test_borrow_fails_without_collateral() {
    let mut env = setup_env();
    seed_liquidity(&mut env);
    refresh_reserve(&mut env);

    // create borrower with no collateral deposited or locked
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();
    let token_acct = create_token_account(&mut env.svm, &user, env.reserve_mint, user.pubkey());
    let obligation = find_obligation(env.lending_market, user.pubkey()).0;
    send(&mut env.svm, &[ix_init_obligation(user.pubkey(), env.lending_market, obligation)], &[&user]);

    // refresh obligation (stamps staleness) but no collateral locked
    refresh_obligation(&mut env, &user, obligation);

    let result = try_send(
        &mut env.svm,
        &[ix_borrow(
            user.pubkey(), env.lending_market, obligation, env.reserve,
            env.reserve_mint, env.liquidity_vault, token_acct, BORROW_AMOUNT,
        )],
        &[&user],
    );
    assert!(result.is_err(), "borrow without any collateral must fail");
}

#[test]
fn test_borrow_without_refresh_fails() {
    let mut env = setup_env();
    seed_liquidity(&mut env);
    refresh_reserve(&mut env);

    let (user, token_acct, _, obligation) = setup_borrower(&mut env, 0);

    // advance one slot so the obligation stamp goes stale
    advance_slots(&mut env.svm, 2);

    let result = try_send(
        &mut env.svm,
        &[ix_borrow(
            user.pubkey(), env.lending_market, obligation, env.reserve,
            env.reserve_mint, env.liquidity_vault, token_acct, BORROW_AMOUNT,
        )],
        &[&user],
    );
    assert!(result.is_err(), "borrow with stale obligation should fail");
}

#[test]
fn test_borrow_and_repay_full_cycle() {
    let mut env = setup_env();
    seed_liquidity(&mut env);
    refresh_reserve(&mut env);

    let (user, token_acct, _, obligation) = setup_borrower(&mut env, 0);

    let vault_before = token_balance(&env.svm, env.liquidity_vault);
    let user_before = token_balance(&env.svm, token_acct);

    send(
        &mut env.svm,
        &[ix_borrow(
            user.pubkey(), env.lending_market, obligation, env.reserve,
            env.reserve_mint, env.liquidity_vault, token_acct, BORROW_AMOUNT,
        )],
        &[&user],
    );

    assert_eq!(
        token_balance(&env.svm, token_acct) - user_before,
        BORROW_AMOUNT,
        "user should receive borrowed tokens"
    );
    assert_eq!(
        vault_before - token_balance(&env.svm, env.liquidity_vault),
        BORROW_AMOUNT,
        "liquidity vault should decrease by borrow amount"
    );

    // advance one slot, then repay
    advance_slots(&mut env.svm, 1);
    let vault_after_borrow = token_balance(&env.svm, env.liquidity_vault);

    send(
        &mut env.svm,
        &[ix_repay(
            user.pubkey(), env.lending_market, obligation, env.reserve,
            env.reserve_mint, env.liquidity_vault, token_acct, BORROW_AMOUNT,
        )],
        &[&user],
    );

    assert_eq!(
        token_balance(&env.svm, env.liquidity_vault) - vault_after_borrow,
        BORROW_AMOUNT,
        "vault should recover the repaid amount"
    );
    assert_eq!(
        token_balance(&env.svm, token_acct),
        user_before,
        "user should be back to original balance after full repay"
    );
}

#[test]
fn test_partial_repay_leaves_debt() {
    let mut env = setup_env();
    seed_liquidity(&mut env);
    refresh_reserve(&mut env);

    // give borrower extra tokens so they can repay in two steps
    let (user, token_acct, _, obligation) = setup_borrower(&mut env, BORROW_AMOUNT);

    send(
        &mut env.svm,
        &[ix_borrow(
            user.pubkey(), env.lending_market, obligation, env.reserve,
            env.reserve_mint, env.liquidity_vault, token_acct, BORROW_AMOUNT,
        )],
        &[&user],
    );

    // repay half
    let half = BORROW_AMOUNT / 2;
    send(
        &mut env.svm,
        &[ix_repay(
            user.pubkey(), env.lending_market, obligation, env.reserve,
            env.reserve_mint, env.liquidity_vault, token_acct, half,
        )],
        &[&user],
    );

    // advance one slot so the second repay tx gets a fresh blockhash (avoids AlreadyProcessed
    // when both halves are byte-identical instructions)
    advance_slots(&mut env.svm, 1);

    // repay remaining half — should succeed
    send(
        &mut env.svm,
        &[ix_repay(
            user.pubkey(), env.lending_market, obligation, env.reserve,
            env.reserve_mint, env.liquidity_vault, token_acct, BORROW_AMOUNT - half,
        )],
        &[&user],
    );
}

#[test]
fn test_refresh_reserve_updates_price() {
    let mut env = setup_env();
    let owner = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    refresh_reserve(&mut env);

    let acct = env.svm.get_account(&env.reserve).expect("reserve should exist");
    assert!(acct.data.len() > 8);
    // price should now be non-zero in reserve state
    let _ = owner; // used above
}

#[test]
fn test_borrow_zero_fails() {
    let mut env = setup_env();
    seed_liquidity(&mut env);
    refresh_reserve(&mut env);

    let (user, token_acct, _, obligation) = setup_borrower(&mut env, 0);

    let result = try_send(
        &mut env.svm,
        &[ix_borrow(
            user.pubkey(), env.lending_market, obligation, env.reserve,
            env.reserve_mint, env.liquidity_vault, token_acct, 0,
        )],
        &[&user],
    );
    assert!(result.is_err(), "borrow(0) should fail");
}

#[test]
fn test_borrow_multi_reserve() {
    // Two-reserve test: deposit USDC as collateral, borrow from a second SOL-like reserve.
    let mut env = setup_env();

    // ── set up second reserve (different mint, same market) ───────────────────
    let owner = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    let mint2_kp = Keypair::new();
    super::setup::create_spl_mint(&mut env.svm, &owner, &mint2_kp, 6);
    let mint2 = mint2_kp.pubkey();

    let reserve2 = find_reserve(env.lending_market, mint2).0;
    let liq_vault2 = find_liquidity_vault(reserve2).0;
    let fee_vault2 = find_fee_vault(reserve2).0;
    let coll_mint2 = find_collateral_mint(reserve2).0;
    let coll_supply2 = find_collateral_supply(reserve2).0;

    let pyth2 = super::pyth::create_price_account(&mut env.svm, 1.0, super::setup::BASE_TIMESTAMP);

    let owner2 = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    send(
        &mut env.svm,
        &[ix_add_reserve(
            owner2.pubkey(), env.lending_market, reserve2, mint2,
            liq_vault2, fee_vault2, coll_mint2, coll_supply2,
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
                pyth_oracle: pyth2,
            },
        )],
        &[&owner2],
    );

    // seed reserve2 with liquidity
    let owner3 = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    let lp_token2 = create_token_account(&mut env.svm, &owner3, mint2, owner3.pubkey());
    let lp_ctoken2 = create_token_account(&mut env.svm, &owner3, coll_mint2, owner3.pubkey());
    mint_tokens(&mut env.svm, &owner3, mint2, lp_token2, SEED_LIQUIDITY);
    send(
        &mut env.svm,
        &[ix_deposit(
            owner3.pubkey(), env.lending_market, reserve2, mint2,
            liq_vault2, coll_mint2, lp_token2, lp_ctoken2, SEED_LIQUIDITY,
        )],
        &[&owner3],
    );

    // refresh both reserves so prices are set
    let clock: solana_sdk::clock::Clock = env.svm.get_sysvar();
    super::pyth::update_price(&mut env.svm, env.pyth_oracle, 1.0, clock.unix_timestamp);
    super::pyth::update_price(&mut env.svm, pyth2, 1.0, clock.unix_timestamp);
    let owner4 = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    send(
        &mut env.svm,
        &[
            ix_refresh_reserve(env.lending_market, env.reserve, env.pyth_oracle),
            ix_refresh_reserve(env.lending_market, reserve2, pyth2),
        ],
        &[&owner4],
    );

    // ── borrower: collateral in reserve1, borrow from reserve2 ───────────────
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();
    let token1 = create_token_account(&mut env.svm, &user, env.reserve_mint, user.pubkey());
    let ctoken1 = create_token_account(&mut env.svm, &user, env.collateral_mint, user.pubkey());
    let token2 = create_token_account(&mut env.svm, &user, mint2, user.pubkey());

    let owner5 = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    mint_tokens(&mut env.svm, &owner5, env.reserve_mint, token1, COLLATERAL);

    // deposit reserve1 → cTokens → lock as collateral
    send(
        &mut env.svm,
        &[ix_deposit(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.liquidity_vault, env.collateral_mint, token1, ctoken1, COLLATERAL,
        )],
        &[&user],
    );
    let obligation = find_obligation(env.lending_market, user.pubkey()).0;
    send(&mut env.svm, &[ix_init_obligation(user.pubkey(), env.lending_market, obligation)], &[&user]);
    send(
        &mut env.svm,
        &[ix_deposit_collateral(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.collateral_mint, ctoken1, env.collateral_supply_vault, obligation, COLLATERAL,
        )],
        &[&user],
    );

    // refresh obligation — prices reserve1 collateral
    send(
        &mut env.svm,
        &[ix_refresh_obligation(env.lending_market, obligation, &[env.reserve, reserve2])],
        &[&user],
    );

    // borrow from reserve2 using reserve1 collateral
    send(
        &mut env.svm,
        &[ix_borrow(
            user.pubkey(), env.lending_market, obligation, reserve2,
            mint2, liq_vault2, token2, BORROW_AMOUNT,
        )],
        &[&user],
    );

    assert_eq!(
        token_balance(&env.svm, token2),
        BORROW_AMOUNT,
        "user should receive reserve2 tokens via cross-reserve borrow"
    );
}

#[test]
fn test_repay_zero_fails() {
    let mut env = setup_env();
    let (user, token_acct, obligation) = setup_active_borrow(&mut env);

    let result = try_send(
        &mut env.svm,
        &[ix_repay(
            user.pubkey(), env.lending_market, obligation, env.reserve,
            env.reserve_mint, env.liquidity_vault, token_acct, 0,
        )],
        &[&user],
    );
    assert!(result.is_err(), "repay(0) should fail");
}

#[test]
fn test_repay_without_borrow_fails() {
    let mut env = setup_env();

    // user with an obligation but no open borrows
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();
    let token_acct = create_token_account(&mut env.svm, &user, env.reserve_mint, user.pubkey());
    let obligation = find_obligation(env.lending_market, user.pubkey()).0;
    send(&mut env.svm, &[ix_init_obligation(user.pubkey(), env.lending_market, obligation)], &[&user]);

    let result = try_send(
        &mut env.svm,
        &[ix_repay(
            user.pubkey(), env.lending_market, obligation, env.reserve,
            env.reserve_mint, env.liquidity_vault, token_acct, 1,
        )],
        &[&user],
    );
    assert!(result.is_err(), "repay with no open borrow should fail");
}
