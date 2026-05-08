use super::setup::*;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::pubkey::Pubkey;

// pool depth — large enough that the borrower's borrow doesn't dominate utilization
const SEED_LIQUIDITY: u64 = 100_000_000; // 100 USDC
const COLLATERAL: u64     =  10_000_000; //  10 USDC collateral
// Max borrow at 80% liquidation threshold: 10 × 0.80 = 8 USDC → HF = exactly 1.0
const BORROW_AT_LIMIT: u64 = 8_000_000;

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Seed the pool and call refresh_reserve so market_price_sf is set.
fn seed_and_refresh(env: &mut TestEnv) {
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
    do_refresh_reserve(env);
}

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

/// Build a fully-collateralised borrower: deposit → deposit_collateral → refresh → borrow.
/// Returns (borrower, token_acct, ctoken_acct, obligation).
fn setup_borrower_at_limit(env: &mut TestEnv) -> (Keypair, Pubkey, Pubkey, Pubkey) {
    let user = Keypair::new();
    env.svm.airdrop(&user.pubkey(), 10_000_000_000).unwrap();

    let token_acct = create_token_account(&mut env.svm, &user, env.reserve_mint, user.pubkey());
    let ctoken_acct = create_token_account(&mut env.svm, &user, env.collateral_mint, user.pubkey());
    let owner = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    mint_tokens(&mut env.svm, &owner, env.reserve_mint, token_acct, COLLATERAL);

    // deposit → cTokens
    send(
        &mut env.svm,
        &[ix_deposit(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.liquidity_vault, env.collateral_mint, token_acct, ctoken_acct, COLLATERAL,
        )],
        &[&user],
    );

    // init + lock collateral
    let obligation = find_obligation(env.lending_market, user.pubkey()).0;
    send(
        &mut env.svm,
        &[ix_init_obligation(user.pubkey(), env.lending_market, obligation)],
        &[&user],
    );
    send(
        &mut env.svm,
        &[ix_deposit_collateral(
            user.pubkey(), env.lending_market, env.reserve, env.reserve_mint,
            env.collateral_mint, ctoken_acct, env.collateral_supply_vault, obligation, COLLATERAL,
        )],
        &[&user],
    );

    // refresh obligation to price the collateral
    send(
        &mut env.svm,
        &[ix_refresh_obligation(env.lending_market, obligation, &[env.reserve])],
        &[&user],
    );

    // borrow exactly at the threshold (HF = 1.0)
    send(
        &mut env.svm,
        &[ix_borrow(
            user.pubkey(), env.lending_market, obligation, env.reserve,
            env.reserve_mint, env.liquidity_vault, token_acct, BORROW_AT_LIMIT,
        )],
        &[&user],
    );

    (user, token_acct, ctoken_acct, obligation)
}

/// Make the borrower's position unhealthy: advance 1 slot so interest accrues,
/// then refresh_reserve + refresh_obligation to update market_value_sf.
fn make_unhealthy(env: &mut TestEnv, obligation: Pubkey) {
    advance_slots(&mut env.svm, 1);
    do_refresh_reserve(env);
    let owner = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    send(
        &mut env.svm,
        &[ix_refresh_obligation(env.lending_market, obligation, &[env.reserve])],
        &[&owner],
    );
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn test_healthy_obligation_cannot_be_liquidated() {
    let mut env = setup_env();
    seed_and_refresh(&mut env);
    let (_, token_acct, _, obligation) = setup_borrower_at_limit(&mut env);

    // obligation is at HF = 1.0 (healthy) — liquidation must fail
    let liquidator = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    let liq_ctoken_acct =
        create_token_account(&mut env.svm, &liquidator, env.collateral_mint, liquidator.pubkey());

    let result = try_send(
        &mut env.svm,
        &[ix_liquidate(
            liquidator.pubkey(), env.lending_market, obligation,
            env.reserve, env.reserve_mint, env.liquidity_vault, token_acct,
            env.reserve, env.collateral_mint, env.collateral_supply_vault,
            liq_ctoken_acct, 1_000_000,
        )],
        &[&liquidator],
    );
    assert!(result.is_err(), "healthy obligation must not be liquidatable");
}

#[test]
fn test_unhealthy_obligation_can_be_liquidated() {
    let mut env = setup_env();
    seed_and_refresh(&mut env);
    let (_, _, _, obligation) = setup_borrower_at_limit(&mut env);

    // one slot of interest flips HF < 1.0
    make_unhealthy(&mut env, obligation);

    // liquidator repays 50% of debt (close factor max)
    let liquidator = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    let liq_repay_acct =
        create_token_account(&mut env.svm, &liquidator, env.reserve_mint, liquidator.pubkey());
    let liq_ctoken_acct =
        create_token_account(&mut env.svm, &liquidator, env.collateral_mint, liquidator.pubkey());
    mint_tokens(&mut env.svm, &liquidator, env.reserve_mint, liq_repay_acct, BORROW_AT_LIMIT);

    let repay = BORROW_AT_LIMIT / 2;
    send(
        &mut env.svm,
        &[ix_liquidate(
            liquidator.pubkey(), env.lending_market, obligation,
            env.reserve, env.reserve_mint, env.liquidity_vault, liq_repay_acct,
            env.reserve, env.collateral_mint, env.collateral_supply_vault,
            liq_ctoken_acct, repay,
        )],
        &[&liquidator],
    );

    // liquidator spent repay tokens and received cTokens
    assert!(
        token_balance(&env.svm, liq_repay_acct) < BORROW_AT_LIMIT,
        "liquidator should have spent repay tokens"
    );
    assert!(
        token_balance(&env.svm, liq_ctoken_acct) > 0,
        "liquidator should have received cToken collateral"
    );
}

#[test]
fn test_liquidation_close_factor_caps_at_50_pct() {
    let mut env = setup_env();
    seed_and_refresh(&mut env);
    let (_, _, _, obligation) = setup_borrower_at_limit(&mut env);
    make_unhealthy(&mut env, obligation);

    let liquidator = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    let liq_repay_acct =
        create_token_account(&mut env.svm, &liquidator, env.reserve_mint, liquidator.pubkey());
    let liq_ctoken_acct =
        create_token_account(&mut env.svm, &liquidator, env.collateral_mint, liquidator.pubkey());
    // fund liquidator with MORE than the full debt — program must cap at 50%
    mint_tokens(&mut env.svm, &liquidator, env.reserve_mint, liq_repay_acct, BORROW_AT_LIMIT * 2);

    let before = token_balance(&env.svm, liq_repay_acct);

    // pass full debt as repay_amount — program should only consume 50%
    send(
        &mut env.svm,
        &[ix_liquidate(
            liquidator.pubkey(), env.lending_market, obligation,
            env.reserve, env.reserve_mint, env.liquidity_vault, liq_repay_acct,
            env.reserve, env.collateral_mint, env.collateral_supply_vault,
            liq_ctoken_acct, BORROW_AT_LIMIT * 2,
        )],
        &[&liquidator],
    );

    let spent = before - token_balance(&env.svm, liq_repay_acct);
    // actual_repay = min(repay_amount, max_repay) = min(16M, 50% of ~8M) ≈ 4M
    assert!(
        spent <= BORROW_AT_LIMIT / 2 + 100, // +100 for interest rounding
        "liquidator should not spend more than 50% of debt (close factor), spent {spent}"
    );
}

#[test]
fn test_liquidation_bonus_received() {
    let mut env = setup_env();
    seed_and_refresh(&mut env);
    let (_, _, _, obligation) = setup_borrower_at_limit(&mut env);
    make_unhealthy(&mut env, obligation);

    let liquidator = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    let liq_repay_acct =
        create_token_account(&mut env.svm, &liquidator, env.reserve_mint, liquidator.pubkey());
    let liq_ctoken_acct =
        create_token_account(&mut env.svm, &liquidator, env.collateral_mint, liquidator.pubkey());
    let repay = BORROW_AT_LIMIT / 4; // repay 25% of debt
    mint_tokens(&mut env.svm, &liquidator, env.reserve_mint, liq_repay_acct, repay);

    send(
        &mut env.svm,
        &[ix_liquidate(
            liquidator.pubkey(), env.lending_market, obligation,
            env.reserve, env.reserve_mint, env.liquidity_vault, liq_repay_acct,
            env.reserve, env.collateral_mint, env.collateral_supply_vault,
            liq_ctoken_acct, repay,
        )],
        &[&liquidator],
    );

    // cTokens seized > repay amount (same asset, same price) because of 5% bonus
    let ctokens_received = token_balance(&env.svm, liq_ctoken_acct);
    assert!(
        ctokens_received > repay,
        "liquidator cTokens ({ctokens_received}) should exceed repay amount ({repay}) due to bonus"
    );
}

#[test]
fn test_obligation_state_reduced_after_liquidation() {
    let mut env = setup_env();
    seed_and_refresh(&mut env);
    let (_, _, _, obligation) = setup_borrower_at_limit(&mut env);
    make_unhealthy(&mut env, obligation);

    let collateral_before = token_balance(&env.svm, env.collateral_supply_vault);

    let liquidator = Keypair::from_bytes(&env.owner.to_bytes()).unwrap();
    let liq_repay_acct =
        create_token_account(&mut env.svm, &liquidator, env.reserve_mint, liquidator.pubkey());
    let liq_ctoken_acct =
        create_token_account(&mut env.svm, &liquidator, env.collateral_mint, liquidator.pubkey());
    let repay = BORROW_AT_LIMIT / 2;
    mint_tokens(&mut env.svm, &liquidator, env.reserve_mint, liq_repay_acct, repay);

    send(
        &mut env.svm,
        &[ix_liquidate(
            liquidator.pubkey(), env.lending_market, obligation,
            env.reserve, env.reserve_mint, env.liquidity_vault, liq_repay_acct,
            env.reserve, env.collateral_mint, env.collateral_supply_vault,
            liq_ctoken_acct, repay,
        )],
        &[&liquidator],
    );

    // collateral vault must have shrunk (cTokens seized from it)
    assert!(
        token_balance(&env.svm, env.collateral_supply_vault) < collateral_before,
        "collateral_supply_vault should decrease after liquidation"
    );
    // repay vault must have grown (liquidator deposited debt tokens)
    // liquidity_vault increases because we repaid into it
    let vault_after = token_balance(&env.svm, env.liquidity_vault);
    // initial vault = SEED_LIQUIDITY - BORROW_AT_LIMIT (after borrow)
    let vault_after_borrow = SEED_LIQUIDITY - BORROW_AT_LIMIT;
    assert!(
        vault_after > vault_after_borrow,
        "liquidity_vault should increase after liquidation repayment"
    );
}
