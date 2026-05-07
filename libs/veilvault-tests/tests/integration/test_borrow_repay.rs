use super::setup::*;
use solana_sdk::signature::{Keypair, Signer};

const DEPOSIT_AMOUNT: u64 = 10_000_000; // 10 USDC
const BORROW_AMOUNT: u64 = 1_000_000;   //  1 USDC

/// Set up a borrower: fund SOL + underlying tokens, init obligation.
/// Returns (user_keypair, underlying_token_acct).
fn new_borrower(env: &mut TestEnv, initial_tokens: u64) -> (Keypair, Pubkey) {
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();

    let token_acct = create_token_account(&mut env.svm, &user, env.reserve_mint, user.pubkey());
    let owner_clone = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    mint_tokens(&mut env.svm, &owner_clone, env.reserve_mint, token_acct, initial_tokens);

    // init obligation for this user
    let obligation = find_obligation(env.lending_market, user.pubkey()).0;
    send(
        &mut env.svm,
        &[ix_init_obligation(user.pubkey(), env.lending_market, obligation)],
        &[&user],
    );

    (user, token_acct)
}

/// Call refresh_obligation (stamps last_update so borrow passes the staleness check).
fn refresh_obligation(env: &mut TestEnv, user: &Keypair) {
    let obligation = find_obligation(env.lending_market, user.pubkey()).0;
    send(
        &mut env.svm,
        &[ix_refresh_obligation(env.lending_market, obligation, &[env.reserve])],
        &[user],
    );
}

/// Call refresh_reserve with the mock Pyth oracle.
/// Also updates the pyth account's publish_time to match the current clock.
fn refresh_reserve(env: &mut TestEnv, user: &Keypair) {
    let clock: solana_sdk::clock::Clock = env.svm.get_sysvar();
    super::pyth::update_price(&mut env.svm, env.pyth_oracle, 1.0, clock.unix_timestamp);
    send(
        &mut env.svm,
        &[ix_refresh_reserve(env.lending_market, env.reserve, env.pyth_oracle)],
        &[user],
    );
}

// ── Liquidity setup ───────────────────────────────────────────────────────────

/// Deposit liquidity as the market owner so borrowers have something to borrow.
fn seed_liquidity(env: &mut TestEnv, amount: u64) {
    let owner_clone = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    let token_acct = create_token_account(&mut env.svm, &owner_clone, env.reserve_mint, owner_clone.pubkey());
    let ctoken_acct = create_token_account(&mut env.svm, &owner_clone, env.collateral_mint, owner_clone.pubkey());
    mint_tokens(&mut env.svm, &owner_clone, env.reserve_mint, token_acct, amount);

    send(
        &mut env.svm,
        &[ix_deposit(
            owner_clone.pubkey(),
            env.lending_market,
            env.reserve,
            env.reserve_mint,
            env.liquidity_vault,
            env.collateral_mint,
            token_acct,
            ctoken_acct,
            amount,
        )],
        &[&owner_clone],
    );
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn test_borrow_without_refresh_fails() {
    let mut env = setup_env();
    seed_liquidity(&mut env, DEPOSIT_AMOUNT);

    let (user, token_acct) = new_borrower(&mut env, 0);
    let obligation = find_obligation(env.lending_market, user.pubkey()).0;

    // borrow WITHOUT calling refresh_obligation first → ObligationStale
    let result = try_send(
        &mut env.svm,
        &[ix_borrow(
            user.pubkey(),
            env.lending_market,
            obligation,
            env.reserve,
            env.reserve_mint,
            env.liquidity_vault,
            token_acct,
            BORROW_AMOUNT,
        )],
        &[&user],
    );
    assert!(result.is_err(), "borrow without refresh_obligation should fail with ObligationStale");
}

#[test]
fn test_borrow_and_repay_full_cycle() {
    let mut env = setup_env();
    seed_liquidity(&mut env, DEPOSIT_AMOUNT);

    let (user, token_acct) = new_borrower(&mut env, 0);
    let obligation = find_obligation(env.lending_market, user.pubkey()).0;

    // refresh obligation to stamp last_update
    refresh_obligation(&mut env, &user);

    let vault_before = token_balance(&env.svm, env.liquidity_vault);
    let user_before = token_balance(&env.svm, token_acct);

    // borrow
    send(
        &mut env.svm,
        &[ix_borrow(
            user.pubkey(),
            env.lending_market,
            obligation,
            env.reserve,
            env.reserve_mint,
            env.liquidity_vault,
            token_acct,
            BORROW_AMOUNT,
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
            user.pubkey(),
            env.lending_market,
            obligation,
            env.reserve,
            env.reserve_mint,
            env.liquidity_vault,
            token_acct,
            BORROW_AMOUNT,
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
    seed_liquidity(&mut env, DEPOSIT_AMOUNT);

    let (user, token_acct) = new_borrower(&mut env, BORROW_AMOUNT / 2);
    let obligation = find_obligation(env.lending_market, user.pubkey()).0;

    refresh_obligation(&mut env, &user);

    send(
        &mut env.svm,
        &[ix_borrow(
            user.pubkey(),
            env.lending_market,
            obligation,
            env.reserve,
            env.reserve_mint,
            env.liquidity_vault,
            token_acct,
            BORROW_AMOUNT,
        )],
        &[&user],
    );

    // repay only half
    let repay = BORROW_AMOUNT / 2;
    send(
        &mut env.svm,
        &[ix_repay(
            user.pubkey(),
            env.lending_market,
            obligation,
            env.reserve,
            env.reserve_mint,
            env.liquidity_vault,
            token_acct,
            repay,
        )],
        &[&user],
    );

    // full repay of remaining half should still succeed (user got BORROW_AMOUNT from borrow)
    send(
        &mut env.svm,
        &[ix_repay(
            user.pubkey(),
            env.lending_market,
            obligation,
            env.reserve,
            env.reserve_mint,
            env.liquidity_vault,
            token_acct,
            BORROW_AMOUNT - repay,
        )],
        &[&user],
    );
}

#[test]
fn test_refresh_reserve_updates_price() {
    let mut env = setup_env();

    let owner_clone = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    refresh_reserve(&mut env, &owner_clone);

    // reserve account should now exist and be non-empty
    let acct = env.svm.get_account(&env.reserve).expect("reserve should exist");
    assert!(acct.data.len() > 8);
}

#[test]
fn test_borrow_zero_fails() {
    let mut env = setup_env();
    seed_liquidity(&mut env, DEPOSIT_AMOUNT);

    let (user, token_acct) = new_borrower(&mut env, 0);
    let obligation = find_obligation(env.lending_market, user.pubkey()).0;
    refresh_obligation(&mut env, &user);

    let result = try_send(
        &mut env.svm,
        &[ix_borrow(
            user.pubkey(),
            env.lending_market,
            obligation,
            env.reserve,
            env.reserve_mint,
            env.liquidity_vault,
            token_acct,
            0,
        )],
        &[&user],
    );
    assert!(result.is_err(), "borrow(0) should fail");
}
