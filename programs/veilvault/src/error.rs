use anchor_lang::error_code;

#[error_code]
pub enum LendingError {
    #[msg("Math operation overflow")]
    MathOverflow,
    #[msg("Invalid version found")]
    InvalidVersion,
    #[msg("Invalid protocol fee")]
    InvalidFee,
}
