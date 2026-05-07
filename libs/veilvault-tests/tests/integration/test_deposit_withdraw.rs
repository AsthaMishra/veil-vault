use super::setup::*;
use solana_sdk::signature::{Keypair, Signer};

const DEPOSIT_AMOUNT: u64 = 1_000_000; // 1 USDC (6 decimals)

/// Helper: create a fresh user with a funded underlying token account.
fn new_user(env: &mut TestEnv, initial_tokens: u64) -> (Keypair, /*token_acct*/ Pubkey, /*ctoken_acct*/ Pubkey) {
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();

    let token_acct = create_token_account(&mut env.svm, &user, env.reserve_mint, user.pubkey());
    let ctoken_acct = create_token_account(&mut env.svm, &user, env.collateral_mint, user.pubkey());

    // mint underlying tokens to the user (owner is the mint authority)
    let owner_clone = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    mint_tokens(&mut env.svm, &owner_clone, env.reserve_mint, token_acct, initial_tokens);

    (user, token_acct, ctoken_acct)
}

#[test]
fn test_deposit_moves_tokens_to_vault() {
    let mut env = setup_env();
    let (user, token_acct, ctoken_acct) = new_user(&mut env, DEPOSIT_AMOUNT * 2);

    let before = token_balance(&env.svm, token_acct);

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
            DEPOSIT_AMOUNT,
        )],
        &[&user],
    );

    let after = token_balance(&env.svm, token_acct);
    assert_eq!(before - after, DEPOSIT_AMOUNT, "underlying tokens should leave user wallet");

    let vault_balance = token_balance(&env.svm, env.liquidity_vault);
    assert_eq!(vault_balance, DEPOSIT_AMOUNT, "tokens should be in liquidity_vault");
}

#[test]
fn test_deposit_mints_ctokens() {
    let mut env = setup_env();
    let (user, token_acct, ctoken_acct) = new_user(&mut env, DEPOSIT_AMOUNT * 2);

    let ctoken_before = token_balance(&env.svm, ctoken_acct);

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
            DEPOSIT_AMOUNT,
        )],
        &[&user],
    );

    let ctoken_after = token_balance(&env.svm, ctoken_acct);
    // first deposit: 1:1 exchange rate, so cTokens minted == underlying deposited
    assert_eq!(ctoken_after - ctoken_before, DEPOSIT_AMOUNT, "cTokens should be minted 1:1 on first deposit");
}

#[test]
fn test_deposit_zero_fails() {
    let mut env = setup_env();
    let (user, token_acct, ctoken_acct) = new_user(&mut env, DEPOSIT_AMOUNT);

    let result = try_send(
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
            0,
        )],
        &[&user],
    );
    assert!(result.is_err(), "deposit(0) should fail");
}

#[test]
fn test_withdraw_returns_underlying() {
    let mut env = setup_env();
    let (user, token_acct, ctoken_acct) = new_user(&mut env, DEPOSIT_AMOUNT * 2);

    // deposit first
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
            DEPOSIT_AMOUNT,
        )],
        &[&user],
    );

    let token_after_deposit = token_balance(&env.svm, token_acct);
    let ctoken_after_deposit = token_balance(&env.svm, ctoken_acct);

    // withdraw all cTokens
    send(
        &mut env.svm,
        &[ix_withdraw(
            user.pubkey(),
            env.lending_market,
            env.reserve,
            env.reserve_mint,
            env.liquidity_vault,
            env.collateral_mint,
            ctoken_acct,
            token_acct,
            ctoken_after_deposit,
        )],
        &[&user],
    );

    let token_after_withdraw = token_balance(&env.svm, token_acct);
    let ctoken_after_withdraw = token_balance(&env.svm, ctoken_acct);

    assert_eq!(ctoken_after_withdraw, 0, "all cTokens should be burned");
    assert_eq!(
        token_after_withdraw - token_after_deposit,
        DEPOSIT_AMOUNT,
        "full underlying should be returned"
    );
}

#[test]
fn test_withdraw_more_than_deposited_fails() {
    let mut env = setup_env();
    let (user, token_acct, ctoken_acct) = new_user(&mut env, DEPOSIT_AMOUNT);

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
            DEPOSIT_AMOUNT,
        )],
        &[&user],
    );

    let ctoken_balance = token_balance(&env.svm, ctoken_acct);

    let result = try_send(
        &mut env.svm,
        &[ix_withdraw(
            user.pubkey(),
            env.lending_market,
            env.reserve,
            env.reserve_mint,
            env.liquidity_vault,
            env.collateral_mint,
            ctoken_acct,
            token_acct,
            ctoken_balance + 1, // one more than available
        )],
        &[&user],
    );
    assert!(result.is_err(), "withdrawing more cTokens than held should fail");
}
