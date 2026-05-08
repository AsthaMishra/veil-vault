use super::setup::*;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::pubkey::Pubkey;

const COLLATERAL_AMOUNT: u64 = 5_000_000; // 5 USDC
const SEED_LIQUIDITY: u64 = 20_000_000;   // 20 USDC

/// Deposit underlying tokens, get cTokens.
fn deposit_to_get_ctokens(env: &mut TestEnv, user: &Keypair, token_acct: Pubkey, ctoken_acct: Pubkey, amount: u64) {
    send(
        &mut env.svm,
        &[ix_deposit(
            user.pubkey(),
            env.lending_market,
            env.reserve,
            env.reserve_mint,
            env.liquidity_vault,
            env.collateral_mint,
            token_acct,
            ctoken_acct,
            amount,
        )],
        &[user],
    );
}

/// Run refresh_reserve (updates oracle price in reserve state).
fn refresh_reserve(env: &mut TestEnv) {
    let clock: solana_sdk::clock::Clock = env.svm.get_sysvar();
    super::pyth::update_price(&mut env.svm, env.pyth_oracle, 1.0, clock.unix_timestamp);
    let owner_clone = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    send(
        &mut env.svm,
        &[ix_refresh_reserve(env.lending_market, env.reserve, env.pyth_oracle)],
        &[&owner_clone],
    );
}

/// Create a fresh user with token + ctoken accounts, minted tokens.
fn new_user(env: &mut TestEnv, initial_tokens: u64) -> (Keypair, Pubkey, Pubkey) {
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();
    let token_acct = create_token_account(&mut env.svm, &user, env.reserve_mint, user.pubkey());
    let ctoken_acct = create_token_account(&mut env.svm, &user, env.collateral_mint, user.pubkey());
    let owner_clone = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    mint_tokens(&mut env.svm, &owner_clone, env.reserve_mint, token_acct, initial_tokens);
    (user, token_acct, ctoken_acct)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn test_deposit_collateral_locks_ctokens_in_vault() {
    let mut env = setup_env();
    let (user, token_acct, ctoken_acct) = new_user(&mut env, COLLATERAL_AMOUNT);

    // first get cTokens via deposit
    deposit_to_get_ctokens(&mut env, &user, token_acct, ctoken_acct, COLLATERAL_AMOUNT);

    let ctoken_before = token_balance(&env.svm, ctoken_acct);
    let vault_before = token_balance(&env.svm, env.collateral_supply_vault);

    let obligation = find_obligation(env.lending_market, user.pubkey()).0;
    send(
        &mut env.svm,
        &[ix_init_obligation(user.pubkey(), env.lending_market, obligation)],
        &[&user],
    );

    send(
        &mut env.svm,
        &[ix_deposit_collateral(
            user.pubkey(),
            env.lending_market,
            env.reserve,
            env.reserve_mint,
            env.collateral_mint,
            ctoken_acct,
            env.collateral_supply_vault,
            obligation,
            COLLATERAL_AMOUNT,
        )],
        &[&user],
    );

    assert_eq!(
        token_balance(&env.svm, ctoken_acct),
        ctoken_before - COLLATERAL_AMOUNT,
        "cTokens should leave user wallet"
    );
    assert_eq!(
        token_balance(&env.svm, env.collateral_supply_vault),
        vault_before + COLLATERAL_AMOUNT,
        "cTokens should arrive in collateral_supply_vault"
    );
}

#[test]
fn test_withdraw_collateral_returns_ctokens() {
    let mut env = setup_env();
    let (user, token_acct, ctoken_acct) = new_user(&mut env, COLLATERAL_AMOUNT);

    deposit_to_get_ctokens(&mut env, &user, token_acct, ctoken_acct, COLLATERAL_AMOUNT);

    let obligation = find_obligation(env.lending_market, user.pubkey()).0;
    send(
        &mut env.svm,
        &[ix_init_obligation(user.pubkey(), env.lending_market, obligation)],
        &[&user],
    );

    // lock collateral
    send(
        &mut env.svm,
        &[ix_deposit_collateral(
            user.pubkey(),
            env.lending_market,
            env.reserve,
            env.reserve_mint,
            env.collateral_mint,
            ctoken_acct,
            env.collateral_supply_vault,
            obligation,
            COLLATERAL_AMOUNT,
        )],
        &[&user],
    );

    // refresh so obligation.last_update is current (required by withdraw_collateral)
    refresh_reserve(&mut env);
    send(
        &mut env.svm,
        &[ix_refresh_obligation(env.lending_market, obligation, &[env.reserve])],
        &[&user],
    );

    let ctoken_before = token_balance(&env.svm, ctoken_acct);

    // unlock all collateral (no borrows → always healthy)
    send(
        &mut env.svm,
        &[ix_withdraw_collateral(
            user.pubkey(),
            env.lending_market,
            env.reserve,
            env.reserve_mint,
            env.collateral_mint,
            env.collateral_supply_vault,
            ctoken_acct,
            obligation,
            COLLATERAL_AMOUNT,
        )],
        &[&user],
    );

    assert_eq!(
        token_balance(&env.svm, ctoken_acct),
        ctoken_before + COLLATERAL_AMOUNT,
        "cTokens should return to user wallet"
    );
    assert_eq!(
        token_balance(&env.svm, env.collateral_supply_vault),
        0,
        "collateral vault should be empty"
    );
}

#[test]
fn test_withdraw_collateral_fails_when_would_break_health() {
    let mut env = setup_env();

    // seed pool with liquidity so user can borrow
    let owner_clone = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    let owner_token = create_token_account(&mut env.svm, &owner_clone, env.reserve_mint, owner_clone.pubkey());
    let owner_ctoken = create_token_account(&mut env.svm, &owner_clone, env.collateral_mint, owner_clone.pubkey());
    mint_tokens(&mut env.svm, &owner_clone, env.reserve_mint, owner_token, SEED_LIQUIDITY);
    send(
        &mut env.svm,
        &[ix_deposit(
            owner_clone.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.liquidity_vault, env.collateral_mint, owner_token, owner_ctoken, SEED_LIQUIDITY,
        )],
        &[&owner_clone],
    );

    let (user, token_acct, ctoken_acct) = new_user(&mut env, COLLATERAL_AMOUNT);
    deposit_to_get_ctokens(&mut env, &user, token_acct, ctoken_acct, COLLATERAL_AMOUNT);

    let obligation = find_obligation(env.lending_market, user.pubkey()).0;
    send(
        &mut env.svm,
        &[ix_init_obligation(user.pubkey(), env.lending_market, obligation)],
        &[&user],
    );

    // lock all collateral
    send(
        &mut env.svm,
        &[ix_deposit_collateral(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.collateral_mint, ctoken_acct, env.collateral_supply_vault, obligation,
            COLLATERAL_AMOUNT,
        )],
        &[&user],
    );

    // refresh → price collateral
    refresh_reserve(&mut env);
    send(
        &mut env.svm,
        &[ix_refresh_obligation(env.lending_market, obligation, &[env.reserve])],
        &[&user],
    );

    // borrow up to the threshold (collateral_value = 5M × 80% = 4M; borrow 4M → HF = 1.0)
    let borrow_at_limit = (COLLATERAL_AMOUNT as u128 * 80 / 100) as u64;
    send(
        &mut env.svm,
        &[ix_borrow(
            user.pubkey(), env.lending_market, obligation, env.reserve,
            env.reserve_mint, env.liquidity_vault, token_acct, borrow_at_limit,
        )],
        &[&user],
    );

    // now try to withdraw any collateral — must fail (HF would go below 1.0)
    advance_slots(&mut env.svm, 1);
    refresh_reserve(&mut env);
    send(
        &mut env.svm,
        &[ix_refresh_obligation(env.lending_market, obligation, &[env.reserve])],
        &[&user],
    );

    let result = try_send(
        &mut env.svm,
        &[ix_withdraw_collateral(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.collateral_mint, env.collateral_supply_vault, ctoken_acct, obligation,
            1, // even 1 cToken withdrawal should fail
        )],
        &[&user],
    );
    assert!(result.is_err(), "withdrawing collateral that would break health should fail");
}
