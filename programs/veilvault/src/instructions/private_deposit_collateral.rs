use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};
use arcium_anchor::prelude::*;
use arcium_client::idl::arcium::types::{CallbackAccount, CircuitSource, OffChainCircuitSource};
use arcium_macros::circuit_hash;

use crate::{
    error::LendingError,
    state::{LendingMarket, Obligation, PrivateObligation, Reserve},
    ArciumSignerAccount, COMP_DEF_OFFSET_ADD_COLLATERAL, ID, ID_CONST,
};
use crate::validate_callback_ixs;
use crate::LendingError as ErrorCode;

// ─── Comp-def registration ────────────────────────────────────────────────────

/// One-time admin call: registers the `add_collateral_2` Arcis circuit as OffChain source.
pub fn add_collateral_comp_def(ctx: Context<AddCollateralCompDef>) -> Result<()> {
    init_comp_def(
        ctx.accounts,
        Some(CircuitSource::OffChain(OffChainCircuitSource {
            source: "https://raw.githubusercontent.com/AsthaMishra/veil-vault/main/veilvault/build/add_collateral_2.arcis".to_string(),
            hash: circuit_hash!("add_collateral_2"),
        })),
        None,
    )?;
    Ok(())
}

#[init_computation_definition_accounts("add_collateral_2", payer)]
#[derive(Accounts)]
pub struct AddCollateralCompDef<'info> {
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

// ─── Private deposit collateral ───────────────────────────────────────────────

/// Locks cTokens into the Obligation (same token mechanics as `deposit_collateral`)
/// AND queues an MPC computation to increase the encrypted collateral amount.
///
/// Arguments:
///   `collateral_amount`   — cleartext cToken count (needed for the SPL transfer).
///   `encrypted_amount`    — client-side encryption of (collateral_amount / 10^decimals)
///                           in whole-token units, using the X25519 shared secret with MXE.
///   `encryption_pubkey`   — caller's ephemeral X25519 public key.
///   `encryption_nonce`    — nonce used when encrypting `encrypted_amount`.
///
/// Privacy model: the individual deposit amount is visible on-chain (unavoidable for
/// token transfers), but the cumulative position stored in PrivateObligation is
/// MXE-encrypted and never revealed to anyone except the MXE cluster.
pub fn private_deposit_collateral(
    ctx: Context<PrivateDepositCollateral>,
    computation_offset: u64,
    collateral_amount: u64,
    encrypted_amount: [u8; 32],
    encryption_pubkey: [u8; 32],
    encryption_nonce: u128,
) -> Result<()> {
    require!(collateral_amount > 0, LendingError::InvalidAmount);
    require!(
        ctx.accounts.private_obligation.is_initialized,
        LendingError::PrivateObligationNotInitialized
    );
    require!(
        !ctx.accounts.lending_market.is_paused(),
        LendingError::InvalidConfig
    );

    let reserve_key = ctx.accounts.reserve.key();
    let collateral_mint_decimals = ctx.accounts.collateral_mint.decimals;

    {
        let reserve = ctx.accounts.reserve.load()?;
        require!(reserve.config.is_active(), LendingError::InvalidConfig);
    }

    // Update public Obligation (needed for cleartext interest accrual).
    {
        let mut obligation = ctx.accounts.obligation.load_mut()?;
        obligation.deposit(reserve_key, collateral_amount)?;
    }

    // Token transfer: user_collateral_account → collateral_supply_vault.
    token_interface::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.user_collateral_account.to_account_info(),
                mint: ctx.accounts.collateral_mint.to_account_info(),
                to: ctx.accounts.collateral_supply_vault.to_account_info(),
                authority: ctx.accounts.depositor.to_account_info(),
            },
        ),
        collateral_amount,
        collateral_mint_decimals,
    )?;

    // Update the collateral_reserve field if not yet set.
    if ctx.accounts.private_obligation.collateral_reserve == Pubkey::default() {
        ctx.accounts.private_obligation.collateral_reserve = reserve_key;
    }

    ctx.accounts.sign_pda_account.bump = ctx.bumps.sign_pda_account;

    // ArgBuilder order matches circuit signature:
    //   add_collateral(amount: Enc<Shared, u64>, state: Enc<Mxe, PrivatePosition>)
    let args = ArgBuilder::new()
        .x25519_pubkey(encryption_pubkey)
        .plaintext_u128(encryption_nonce)
        .encrypted_u64(encrypted_amount)
        .plaintext_u128(ctx.accounts.private_obligation.nonce)
        .account(
            ctx.accounts.private_obligation.key(),
            8 + 1,  // discriminator (8) + bump (1) = offset of enc_state field
            64,     // [[u8; 32]; 2] = 64 bytes
        )
        .build();

    queue_computation(
        ctx.accounts,
        computation_offset,
        args,
        vec![AddCollateral2Callback::callback_ix(
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

#[queue_computation_accounts("add_collateral_2", depositor)]
#[derive(Accounts)]
#[instruction(computation_offset: u64)]
pub struct PrivateDepositCollateral<'info> {
    #[account(mut)]
    pub depositor: Signer<'info>,

    #[account(
        init_if_needed,
        space = 9,
        payer = depositor,
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

    #[account(address = derive_comp_def_pda!(COMP_DEF_OFFSET_ADD_COLLATERAL))]
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
        seeds = [b"reserve", lending_market.key().as_ref(), reserve_mint.key().as_ref()],
        bump,
    )]
    pub reserve: AccountLoader<'info, Reserve>,

    pub reserve_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        seeds = [b"collateral_mint", reserve.key().as_ref()],
        bump,
    )]
    pub collateral_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        token::mint = collateral_mint,
        token::authority = depositor,
    )]
    pub user_collateral_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [b"collateral_supply", reserve.key().as_ref()],
        bump,
    )]
    pub collateral_supply_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [b"obligation", lending_market.key().as_ref(), depositor.key().as_ref()],
        bump,
    )]
    pub obligation: AccountLoader<'info, Obligation>,

    #[account(
        mut,
        seeds = [
            b"private_obligation",
            lending_market.key().as_ref(),
            depositor.key().as_ref(),
        ],
        bump = private_obligation.bump,
        constraint = private_obligation.owner == depositor.key() @ LendingError::InvalidConfig,
    )]
    pub private_obligation: Box<Account<'info, PrivateObligation>>,

    pub token_program: Interface<'info, TokenInterface>,
}

// ─── Callback ─────────────────────────────────────────────────────────────────

/// Stores the updated encrypted state after the MXE runs add_collateral_2.
#[arcium_callback(encrypted_ix = "add_collateral_2")]
pub fn add_collateral_2_callback(
    ctx: Context<AddCollateral2Callback>,
    output: SignedComputationOutputs<AddCollateral2Output>,
) -> Result<()> {
    let enc = match output.verify_output(
        &ctx.accounts.cluster_account,
        &ctx.accounts.computation_account,
    ) {
        Ok(AddCollateral2Output { field_0 }) => field_0,
        Err(_) => return Err(LendingError::AbortedComputation.into()),
    };

    ctx.accounts.private_obligation.enc_state = enc.ciphertexts;
    ctx.accounts.private_obligation.nonce = enc.nonce;

    Ok(())
}

#[callback_accounts("add_collateral_2")]
#[derive(Accounts)]
pub struct AddCollateral2Callback<'info> {
    pub arcium_program: Program<'info, Arcium>,

    #[account(address = derive_comp_def_pda!(COMP_DEF_OFFSET_ADD_COLLATERAL))]
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
