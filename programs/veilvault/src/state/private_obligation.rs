use anchor_lang::prelude::*;

/// Per-user confidential position, mirroring the public Obligation but with amounts
/// stored as Arcium MXE ciphertexts instead of plaintext u64/u128 values.
///
/// Field layout (Borsh, no padding):
///   discriminator:     8 bytes  (Anchor prepends, not in struct)
///   bump:              1 byte   offset 8
///   enc_state:        64 bytes  offset 9   ← [[u8;32];2], MXE reads via ArgBuilder.account()
///   nonce:            16 bytes  offset 73  ← u128 state nonce, rotated by MXE on every write
///   owner:            32 bytes  offset 89
///   lending_market:   32 bytes  offset 121
///   collateral_reserve: 32 bytes offset 153
///   borrow_reserve:   32 bytes  offset 185
///   is_liquidatable:   1 byte   offset 217
///   is_initialized:    1 byte   offset 218
///   total:           219 bytes + 8 = 227 with discriminator
#[account]
#[derive(InitSpace)]
pub struct PrivateObligation {
    pub bump: u8,
    /// MXE-encrypted PrivatePosition: [collateral_tokens_ciphertext, borrow_tokens_ciphertext].
    /// Each field is one 32-byte Rescue-cipher ciphertext.
    pub enc_state: [[u8; 32]; 2],
    /// Nonce rotated by the MXE on every state update; prevents replay attacks.
    pub nonce: u128,
    pub owner: Pubkey,
    pub lending_market: Pubkey,
    /// Reserve whose cTokens back the collateral side of this position.
    pub collateral_reserve: Pubkey,
    /// Reserve from which tokens were borrowed.
    pub borrow_reserve: Pubkey,
    /// Set to true by check_health_callback when HF < 1. Cleared after liquidation.
    pub is_liquidatable: bool,
    /// True after init_position_callback completes; gates all other private ops.
    pub is_initialized: bool,
}

/// Emitted when the MXE determines a position is under-collateralised.
/// Liquidators listen for this event and then call execute_private_liquidation.
#[event]
pub struct LiquidatableEvent {
    pub private_obligation: Pubkey,
    pub owner: Pubkey,
}
