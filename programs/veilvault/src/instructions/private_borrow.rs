use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};
use arcium_anchor::prelude::*;
use arcium_client::idl::arcium::types::{CallbackAccount, CircuitSource, OffChainCircuitSource};
use arcium_macros::circuit_hash;

use crate::{
    error::LendingError,
    state::{LendingMarket, Obligation, PrivateObligation, Reserve},
    ArciumSignerAccount, COMP_DEF_OFFSET_ADD_BORROW, ID, ID_CONST,
};
use crate::validate_callback_ixs;
use crate::LendingError as ErrorCode;

// ─── Comp-def registration ────────────────────────────────────────────────────

/// One-time admin call: registers the `add_borrow_2` Arcis circuit as OffChain source.
pub fn add_borrow_comp_def(ctx: Context<AddBorrowCompDef>) -> Result<()> {
    init_comp_def(
        ctx.accounts,
        Some(CircuitSource::OffChain(OffChainCircuitSource {
            source: "https://raw.githubusercontent.com/AsthaMishra/veil-vault/main/veilvault/build/add_borrow_2.arcis".to_string(),
            hash: circuit_hash!("add_borrow_2"),
        })),
        None,
    )?;
    Ok(())
}

#[init_computation_definition_accounts("add_borrow_2", payer)]
#[derive(Accounts)]
pub struct AddBorrowCompDef<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(mut, address = derive_mxe_pda!())]
    pub mxe_account: Box<Account<'info, MXEAccount>>,
    /// CHECK: initialised by arcium program.
    #[account(mut)]
    pub comp_def_account: UncheckedAccount<'info>,
    /// CHECK: address_lookup_table, checked by arcium program.
    #[account(mut, address = derive_mxe_lut_pda!(mxe_account.lut_offset_slot))]
    pub address_lookup_table: UncheckedAccount<'info>,
    #[account(address = LUT_PROGRAM_ID)]
    /// CHECK: lut_program is the Address Lookup Table program.
    pub lut_program: UncheckedAccount<'info>,
    pub arcium_program: Program<'info, Arcium>,
    pub system_program: Program<'info, System>,
}

// ─── Private borrow ───────────────────────────────────────────────────────────

/// Transfers underlying tokens from the liquidity vault to the borrower
/// AND queues an MPC computation to increase the encrypted borrow amount.
///
/// The plaintext health check (HF >= 1 via public Obligation) still runs before
/// tokens are released. The MPC encrypted health check is used by liquidators
/// who call `private_check_liquidatable` — it does not gate this instruction.
///
/// Arguments:
///   `amount`            — cleartext token amount (for SPL transfer).
///   `encrypted_amount`  — client-side encryption of (amount / 10^decimals) in whole-token units.
///   `encryption_pubkey` — caller's ephemeral X25519 public key.
///   `encryption_nonce`  — nonce used when encrypting.
pub fn private_borrow(
    ctx: Context<PrivateBorrow>,
    computation_offset: u64,
    amount: u64,
    encrypted_amount: [u8; 32],
    encryption_pubkey: [u8; 32],
    encryption_nonce: u128,
) -> Result<()> {
    require!(amount > 0, LendingError::InvalidAmount);
    require!(
        ctx.accounts.private_obligation.is_initialized,
        LendingError::PrivateObligationNotInitialized
    );
    require!(
        !ctx.accounts.lending_market.is_paused(),
        LendingError::InvalidConfig
    );

    let clock = Clock::get()?;
    let lending_market_owner = ctx.accounts.lending_market.owner;
    let lending_market_bump = ctx.accounts.lending_market.bump;
    let reserve_mint_decimals = ctx.accounts.reserve_mint.decimals;
    let reserve_key = ctx.accounts.reserve.key();

    let (cumulative_borrow_rate_sf, reserve_price_sf) = {
        let mut reserve = ctx.accounts.reserve.load_mut()?;
        require!(reserve.config.is_active(), LendingError::InvalidConfig);
        require!(reserve.liquidity.market_price_sf > 0, LendingError::PriceNotValid);
        reserve.accrue_interest(clock.slot)?;
        reserve.borrow(amount)?;
        (
            reserve.liquidity.cumulative_borrow_rate_sf,
            reserve.liquidity.market_price_sf,
        )
    };

    {
        let mut obligation = ctx.accounts.obligation.load_mut()?;
        require!(
            !obligation.last_update.is_slot_stale(clock.slot),
            LendingError::ObligationStale
        );

        if let Ok(slot_idx) = obligation.find_borrow(reserve_key) {
            obligation.accrue_interest(slot_idx, cumulative_borrow_rate_sf)?;
        }
        obligation.borrow(reserve_key, amount as u128, cumulative_borrow_rate_sf)?;

        // Inline health-factor check (public collateral vs new borrow + existing borrows).
        let collateral_value_sf: u128 = obligation.deposits
            [..obligation.deposits_count as usize]
            .iter()
            .filter(|d| d.is_active())
            .map(|d| d.market_value_sf)
            .sum();

        let existing_borrow_sf: u128 = obligation.borrows
            [..obligation.borrows_count as usize]
            .iter()
            .filter(|b| b.is_active())
            .map(|b| b.market_value_sf)
            .sum();

        let new_borrow_sf = (amount as u128)
            .checked_mul(reserve_price_sf)
            .ok_or(LendingError::MathOverflow)?;

        let total_borrow_sf = existing_borrow_sf
            .checked_add(new_borrow_sf)
            .ok_or(LendingError::MathOverflow)?;

        require!(
            collateral_value_sf >= total_borrow_sf,
            LendingError::UnhealthyObligation
        );
    }

    // SPL transfer: liquidity_vault → borrower (lending_market PDA signs).
    let signer_seeds: &[&[&[u8]]] = &[&[
        b"lending_market",
        lending_market_owner.as_ref(),
        &[lending_market_bump],
    ]];

    token_interface::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.liquidity_vault.to_account_info(),
                mint: ctx.accounts.reserve_mint.to_account_info(),
                to: ctx.accounts.user_token_account.to_account_info(),
                authority: ctx.accounts.lending_market.to_account_info(),
            },
            signer_seeds,
        ),
        amount,
        reserve_mint_decimals,
    )?;

    // Track which reserve the borrow came from.
    if ctx.accounts.private_obligation.borrow_reserve == Pubkey::default() {
        ctx.accounts.private_obligation.borrow_reserve = reserve_key;
    }

    ctx.accounts.sign_pda_account.bump = ctx.bumps.sign_pda_account;

    // ArgBuilder order matches circuit signature:
    //   add_borrow(amount: Enc<Shared, u64>, state: Enc<Mxe, PrivatePosition>)
    let args = ArgBuilder::new()
        .x25519_pubkey(encryption_pubkey)
        .plaintext_u128(encryption_nonce)
        .encrypted_u64(encrypted_amount)
        .plaintext_u128(ctx.accounts.private_obligation.nonce)
        .account(
            ctx.accounts.private_obligation.key(),
            8 + 1,  // discriminator (8) + bump (1) = offset of enc_state
            64,     // [[u8; 32]; 2] = 64 bytes
        )
        .build();

    queue_computation(
        ctx.accounts,
        computation_offset,
        args,
        vec![AddBorrow2Callback::callback_ix(
            computation_offset,
            &ctx.accounts.mxe_account,
            &[CallbackAccount {
                pubkey: ctx.accounts.private_obligation.key(),
                is_writable: true,
            }],
        )?],
        1,
        0,
    )?;

    Ok(())
}

#[queue_computation_accounts("add_borrow_2", borrower)]
#[derive(Accounts)]
#[instruction(computation_offset: u64)]
pub struct PrivateBorrow<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,

    #[account(
        init_if_needed,
        space = 9,
        payer = borrower,
        seeds = [&SIGN_PDA_SEED],
        bump,
        address = derive_sign_pda!(),
    )]
    pub sign_pda_account: Account<'info, ArciumSignerAccount>,

    #[account(address = derive_mxe_pda!())]
    pub mxe_account: Box<Account<'info, MXEAccount>>,

    #[account(
        mut,
        address = derive_mempool_pda!(mxe_account, LendingError::ClusterNotSet)
    )]
    /// CHECK: checked by arcium program.
    pub mempool_account: UncheckedAccount<'info>,

    #[account(
        mut,
        address = derive_execpool_pda!(mxe_account, LendingError::ClusterNotSet)
    )]
    /// CHECK: checked by arcium program.
    pub executing_pool: UncheckedAccount<'info>,

    #[account(
        mut,
        address = derive_comp_pda!(computation_offset, mxe_account, LendingError::ClusterNotSet)
    )]
    /// CHECK: checked by arcium program.
    pub computation_account: UncheckedAccount<'info>,

    #[account(address = derive_comp_def_pda!(COMP_DEF_OFFSET_ADD_BORROW))]
    pub comp_def_account: Box<Account<'info, ComputationDefinitionAccount>>,

    #[account(
        mut,
        address = derive_cluster_pda!(mxe_account, LendingError::ClusterNotSet)
    )]
    pub cluster_account: Box<Account<'info, Cluster>>,

    #[account(mut, address = ARCIUM_FEE_POOL_ACCOUNT_ADDRESS)]
    pub pool_account: Box<Account<'info, FeePool>>,

    #[account(mut, address = ARCIUM_CLOCK_ACCOUNT_ADDRESS)]
    pub clock_account: Box<Account<'info, ClockAccount>>,

    pub system_program: Program<'info, System>,
    pub arcium_program: Program<'info, Arcium>,

    // VeilVault accounts
    #[account(
        seeds = [b"lending_market", lending_market.owner.as_ref()],
        bump = lending_market.bump,
    )]
    pub lending_market: Box<Account<'info, LendingMarket>>,

    #[account(
        mut,
        seeds = [b"obligation", lending_market.key().as_ref(), borrower.key().as_ref()],
        bump,
    )]
    pub obligation: AccountLoader<'info, Obligation>,

    #[account(
        mut,
        seeds = [b"reserve", lending_market.key().as_ref(), reserve_mint.key().as_ref()],
        bump,
    )]
    pub reserve: AccountLoader<'info, Reserve>,

    pub reserve_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        seeds = [b"liquidity_vault", reserve.key().as_ref()],
        bump,
    )]
    pub liquidity_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = reserve_mint,
    )]
    pub user_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [
            b"private_obligation",
            lending_market.key().as_ref(),
            borrower.key().as_ref(),
        ],
        bump = private_obligation.bump,
    )]
    pub private_obligation: Box<Account<'info, PrivateObligation>>,

    pub token_program: Interface<'info, TokenInterface>,
}

// ─── Callback ─────────────────────────────────────────────────────────────────

/// Stores the updated encrypted state after the MXE runs add_borrow_2.
#[arcium_callback(encrypted_ix = "add_borrow_2")]
pub fn add_borrow_2_callback(
    ctx: Context<AddBorrow2Callback>,
    output: SignedComputationOutputs<AddBorrow2Output>,
) -> Result<()> {
    let enc = match output.verify_output(
        &ctx.accounts.cluster_account,
        &ctx.accounts.computation_account,
    ) {
        Ok(AddBorrow2Output { field_0 }) => field_0,
        Err(_) => return Err(LendingError::AbortedComputation.into()),
    };

    ctx.accounts.private_obligation.enc_state = enc.ciphertexts;
    ctx.accounts.private_obligation.nonce = enc.nonce;

    Ok(())
}

#[callback_accounts("add_borrow_2")]
#[derive(Accounts)]
pub struct AddBorrow2Callback<'info> {
    pub arcium_program: Program<'info, Arcium>,

    #[account(address = derive_comp_def_pda!(COMP_DEF_OFFSET_ADD_BORROW))]
    pub comp_def_account: Box<Account<'info, ComputationDefinitionAccount>>,

    #[account(address = derive_mxe_pda!())]
    pub mxe_account: Box<Account<'info, MXEAccount>>,

    /// CHECK: checked by arcium program via callback context constraints.
    pub computation_account: UncheckedAccount<'info>,

    #[account(address = derive_cluster_pda!(mxe_account, LendingError::ClusterNotSet))]
    pub cluster_account: Box<Account<'info, Cluster>>,

    #[account(address = ::anchor_lang::solana_program::sysvar::instructions::ID)]
    /// CHECK: instructions sysvar.
    pub instructions_sysvar: AccountInfo<'info>,

    #[account(mut)]
    pub private_obligation: Account<'info, PrivateObligation>,
}
