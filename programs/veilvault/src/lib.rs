use anchor_lang::prelude::*;
use arcium_anchor::prelude::*;

pub mod constants;
pub mod error;
pub mod instructions;
pub mod state;
pub mod utils;

pub use error::*;
use instructions::*;
use state::InitReserveConfigParams;

declare_id!("CMbnY6XXekgVZvFHwmB6yC15TD5x7anD1XmHrm218Wbs");


// ── Arcium computation-definition offsets ──────────────────────────────────────
// Each offset is a deterministic u32 derived from the circuit function name.
// These must match the names in encrypted-ixs/src/lib.rs exactly.
pub const COMP_DEF_OFFSET_INIT_POSITION: u32 = comp_def_offset("init_position_2");
pub const COMP_DEF_OFFSET_ADD_COLLATERAL: u32 = comp_def_offset("add_collateral_2");
pub const COMP_DEF_OFFSET_ADD_BORROW: u32 = comp_def_offset("add_borrow_2");
pub const COMP_DEF_OFFSET_CHECK_HEALTH: u32 = comp_def_offset("check_health");

#[arcium_program]
pub mod veilvault {
    use super::*;

    // ── Cleartext lending instructions ────────────────────────────────────────

    pub fn initialize_market(
        ctx: Context<InitializeMarket>,
        args: InitializeMarketArgs,
    ) -> Result<()> {
        instructions::initialize_market::initialize_market(ctx, args)
    }

    pub fn add_reserve(ctx: Context<AddReserve>, args: AddReserveArgs) -> Result<()> {
        instructions::add_reserve::add_reserve(ctx, args)
    }

    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        instructions::deposit::deposit(ctx, amount)
    }

    pub fn deposit_collateral(
        ctx: Context<DepositCollateral>,
        collateral_amount: u64,
    ) -> Result<()> {
        instructions::deposit_collateral::deposit_collateral(ctx, collateral_amount)
    }

    pub fn withdraw_collateral(
        ctx: Context<WithdrawCollateral>,
        collateral_amount: u64,
    ) -> Result<()> {
        instructions::withdraw_collateral::withdraw_collateral(ctx, collateral_amount)
    }

    pub fn init_obligation(ctx: Context<InitObligation>) -> Result<()> {
        instructions::init_obligation::init_obligation(ctx)
    }

    pub fn borrow(ctx: Context<Borrow>, amount: u64) -> Result<()> {
        instructions::borrow::borrow(ctx, amount)
    }

    pub fn repay(ctx: Context<Repay>, amount: u64) -> Result<()> {
        instructions::repay::repay(ctx, amount)
    }

    pub fn withdraw(ctx: Context<Withdraw>, collateral_amount: u64) -> Result<()> {
        instructions::withdraw::withdraw(ctx, collateral_amount)
    }

    pub fn refresh_reserve(ctx: Context<RefreshReserve>) -> Result<()> {
        instructions::refresh_reserve::refresh_reserve(ctx)
    }

    pub fn refresh_obligation(ctx: Context<RefreshObligation>) -> Result<()> {
        instructions::refresh_obligation::refresh_obligation(ctx)
    }

    pub fn liquidate(ctx: Context<Liquidate>, repay_amount: u64) -> Result<()> {
        instructions::liquidate::liquidate(ctx, repay_amount)
    }

    pub fn set_pause(ctx: Context<SetPause>, paused: bool) -> Result<()> {
        instructions::set_pause::set_pause(ctx, paused)
    }

    pub fn update_reserve_config(
        ctx: Context<UpdateReserveConfig>,
        new_config: InitReserveConfigParams,
    ) -> Result<()> {
        instructions::update_reserve_config::update_reserve_config(ctx, new_config)
    }

    pub fn update_market_authority(
        ctx: Context<UpdateMarketAuthority>,
        new_authority: Pubkey,
    ) -> Result<()> {
        instructions::update_market_authority::update_market_authority(ctx, new_authority)
    }

    // ── Arcium circuit registration (one-time admin calls) ────────────────────

    pub fn init_position_comp_def(ctx: Context<InitPositionCompDef>) -> Result<()> {
        instructions::init_private_obligation::init_position_comp_def(ctx)
    }

    pub fn add_collateral_comp_def(ctx: Context<AddCollateralCompDef>) -> Result<()> {
        instructions::private_deposit_collateral::add_collateral_comp_def(ctx)
    }

    pub fn add_borrow_comp_def(ctx: Context<AddBorrowCompDef>) -> Result<()> {
        instructions::private_borrow::add_borrow_comp_def(ctx)
    }

    pub fn check_health_comp_def(ctx: Context<CheckHealthCompDef>) -> Result<()> {
        instructions::private_liquidate::check_health_comp_def(ctx)
    }

    // ── Arcium private lending instructions ───────────────────────────────────

    /// Create a PrivateObligation PDA and queue initial encrypted state from MXE.
    pub fn init_private_obligation(
        ctx: Context<InitPrivateObligation>,
        computation_offset: u64,
    ) -> Result<()> {
        instructions::init_private_obligation::init_private_obligation(ctx, computation_offset)
    }

    /// Lock cTokens as collateral AND update the MXE-encrypted collateral amount.
    pub fn private_deposit_collateral(
        ctx: Context<PrivateDepositCollateral>,
        computation_offset: u64,
        collateral_amount: u64,
        encrypted_amount: [u8; 32],
        encryption_pubkey: [u8; 32],
        encryption_nonce: u128,
    ) -> Result<()> {
        instructions::private_deposit_collateral::private_deposit_collateral(
            ctx,
            computation_offset,
            collateral_amount,
            encrypted_amount,
            encryption_pubkey,
            encryption_nonce,
        )
    }

    /// Borrow underlying tokens AND update the MXE-encrypted borrow amount.
    pub fn private_borrow(
        ctx: Context<PrivateBorrow>,
        computation_offset: u64,
        amount: u64,
        encrypted_amount: [u8; 32],
        encryption_pubkey: [u8; 32],
        encryption_nonce: u128,
    ) -> Result<()> {
        instructions::private_borrow::private_borrow(
            ctx,
            computation_offset,
            amount,
            encrypted_amount,
            encryption_pubkey,
            encryption_nonce,
        )
    }

    /// Ask the MXE to check whether a position is liquidatable (health factor < 1).
    /// The result arrives via check_health_callback.
    pub fn private_check_liquidatable(
        ctx: Context<PrivateCheckLiquidatable>,
        computation_offset: u64,
        exchange_rate_bps: u64,
        collateral_price_cents: u64,
        borrow_price_cents: u64,
        ltv_bps: u64,
    ) -> Result<()> {
        instructions::private_liquidate::private_check_liquidatable(
            ctx,
            computation_offset,
            exchange_rate_bps,
            collateral_price_cents,
            borrow_price_cents,
            ltv_bps,
        )
    }

    /// Seize collateral from a position already flagged as liquidatable by check_health_callback.
    pub fn execute_private_liquidation(
        ctx: Context<ExecutePrivateLiquidation>,
        repay_amount: u64,
    ) -> Result<()> {
        instructions::private_liquidate::execute_private_liquidation(ctx, repay_amount)
    }

    // ── Arcium callbacks (invoked by MXE nodes, not by users) ────────────────

    #[arcium_callback(encrypted_ix = "init_position_2")]
    pub fn init_position_2_callback(
        ctx: Context<InitPosition2Callback>,
        output: SignedComputationOutputs<InitPosition2Output>,
    ) -> Result<()> {
        instructions::init_private_obligation::init_position_2_callback(ctx, output)
    }

    #[arcium_callback(encrypted_ix = "add_collateral_2")]
    pub fn add_collateral_2_callback(
        ctx: Context<AddCollateral2Callback>,
        output: SignedComputationOutputs<AddCollateral2Output>,
    ) -> Result<()> {
        instructions::private_deposit_collateral::add_collateral_2_callback(ctx, output)
    }

    #[arcium_callback(encrypted_ix = "add_borrow_2")]
    pub fn add_borrow_2_callback(
        ctx: Context<AddBorrow2Callback>,
        output: SignedComputationOutputs<AddBorrow2Output>,
    ) -> Result<()> {
        instructions::private_borrow::add_borrow_2_callback(ctx, output)
    }

    #[arcium_callback(encrypted_ix = "check_health")]
    pub fn check_health_callback(
        ctx: Context<CheckHealthCallback>,
        output: SignedComputationOutputs<CheckHealthOutput>,
    ) -> Result<()> {
        instructions::private_liquidate::check_health_callback(ctx, output)
    }
}
