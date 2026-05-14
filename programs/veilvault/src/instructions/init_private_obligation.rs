use anchor_lang::prelude::*;
use arcium_anchor::prelude::*;
use arcium_client::idl::arcium::types::{CallbackAccount, CircuitSource, OffChainCircuitSource};
use arcium_macros::circuit_hash;

use crate::{
    error::LendingError,
    state::{LendingMarket, PrivateObligation},
    ArciumSignerAccount, COMP_DEF_OFFSET_INIT_POSITION, ID, ID_CONST,
};
use crate::validate_callback_ixs;
use crate::LendingError as ErrorCode;

// ─── Comp-def registration ────────────────────────────────────────────────────

/// One-time admin call: registers the `init_position_2` Arcis circuit as OffChain source.
/// Must be called once per cluster before any PrivateObligation can be created.
pub fn init_position_comp_def(ctx: Context<InitPositionCompDef>) -> Result<()> {
    init_comp_def(
        ctx.accounts,
        Some(CircuitSource::OffChain(OffChainCircuitSource {
            source: "https://raw.githubusercontent.com/AsthaMishra/veil-vault/main/veilvault/build/init_position_2.arcis".to_string(),
            hash: circuit_hash!("init_position_2"),
        })),
        None,
    )?;
    Ok(())
}

#[init_computation_definition_accounts("init_position_2", payer)]
#[derive(Accounts)]
pub struct InitPositionCompDef<'info> {
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

// ─── Init obligation ──────────────────────────────────────────────────────────

/// Creates a PrivateObligation PDA for the caller and queues the MXE's
/// `init_position` computation to initialise the encrypted state.
///
/// Safe to re-call if a previous computation expired before the callback fired
/// (is_initialized = false). Errors if already fully initialised.
pub fn init_private_obligation(
    ctx: Context<InitPrivateObligation>,
    computation_offset: u64,
) -> Result<()> {
    require!(
        !ctx.accounts.private_obligation.is_initialized,
        LendingError::AlreadyInitialized
    );

    let po = &mut ctx.accounts.private_obligation;
    po.bump = ctx.bumps.private_obligation;
    po.owner = ctx.accounts.payer.key();
    po.lending_market = ctx.accounts.lending_market.key();
    po.collateral_reserve = Pubkey::default();
    po.borrow_reserve = Pubkey::default();
    po.is_initialized = false;
    po.is_liquidatable = false;

    ctx.accounts.sign_pda_account.bump = ctx.bumps.sign_pda_account;

    let args = ArgBuilder::new().build();

    queue_computation(
        ctx.accounts,
        computation_offset,
        args,
        vec![InitPosition2Callback::callback_ix(
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

#[queue_computation_accounts("init_position_2", payer)]
#[derive(Accounts)]
#[instruction(computation_offset: u64)]
pub struct InitPrivateObligation<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        init_if_needed,
        space = 9,
        payer = payer,
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

    #[account(address = derive_comp_def_pda!(COMP_DEF_OFFSET_INIT_POSITION))]
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
        init_if_needed,
        payer = payer,
        space = 8 + PrivateObligation::INIT_SPACE,
        seeds = [
            b"private_obligation",
            lending_market.key().as_ref(),
            payer.key().as_ref(),
        ],
        bump,
    )]
    pub private_obligation: Box<Account<'info, PrivateObligation>>,
}

// ─── Callback ─────────────────────────────────────────────────────────────────

/// Receives the MXE-encrypted initial state and stores it in PrivateObligation.
#[arcium_callback(encrypted_ix = "init_position_2")]
pub fn init_position_2_callback(
    ctx: Context<InitPosition2Callback>,
    output: SignedComputationOutputs<InitPosition2Output>,
) -> Result<()> {
    let is_failure = matches!(output, SignedComputationOutputs::Failure(_));
    msg!("init_position_2_callback: is_failure={}", is_failure);

    let enc = match output.verify_output(
        &ctx.accounts.cluster_account,
        &ctx.accounts.computation_account,
    ) {
        Ok(InitPosition2Output { field_0 }) => {
            msg!("verify_output: Success");
            field_0
        }
        Err(e) => {
            msg!("verify_output: Err {:?}", e);
            return Err(LendingError::AbortedComputation.into());
        }
    };

    ctx.accounts.private_obligation.enc_state = enc.ciphertexts;
    ctx.accounts.private_obligation.nonce = enc.nonce;
    ctx.accounts.private_obligation.is_initialized = true;

    Ok(())
}

#[callback_accounts("init_position_2")]
#[derive(Accounts)]
pub struct InitPosition2Callback<'info> {
    pub arcium_program: Program<'info, Arcium>,

    #[account(address = derive_comp_def_pda!(COMP_DEF_OFFSET_INIT_POSITION))]
    pub comp_def_account: Box<Account<'info, ComputationDefinitionAccount>>,

    #[account(address = derive_mxe_pda!())]
    pub mxe_account: Box<Account<'info, MXEAccount>>,

    /// CHECK: checked by arcium program via callback context constraints.
    pub computation_account: UncheckedAccount<'info>,

    #[account(address = derive_cluster_pda!(mxe_account, LendingError::ClusterNotSet))]
    pub cluster_account: Box<Account<'info, Cluster>>,

    #[account(address = ::anchor_lang::solana_program::sysvar::instructions::ID)]
    /// CHECK: instructions sysvar checked by the account address constraint.
    pub instructions_sysvar: AccountInfo<'info>,

    #[account(mut)]
    pub private_obligation: Account<'info, PrivateObligation>,
}
