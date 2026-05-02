use anchor_lang::error_code;

#[error_code]
pub enum LendingError {
    #[msg("Math operation overflow")]
    MathOverflow,
    #[msg("Invalid version found")]
    InvalidVersion,
    #[msg("Invalid protocol fee")]
    InvalidFee,
    #[msg("Reserve does not have sufficient liquidity")]
    InsufficientLiquidity,
    #[msg("Borrow rate cannot be zero")]
    BorrowRateZeroFound,
    #[msg("Slots not elapsed yet")]
    ZeroSlotsElapsed,
}
