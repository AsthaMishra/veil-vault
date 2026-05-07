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
    #[msg("Invalid reserve config value")]
    InvalidConfig,
    #[msg("Deposit limit exceeded")]
    DepositLimitExceeded,
    #[msg("Borrow limit exceeded")]
    BorrowLimitExceeded,
    #[msg("Insufficient collateral supply to burn")]
    InsufficientCollateral,
    #[msg("No empty deposit slot available in obligation")]
    ObligationDepositsFull,
    #[msg("No empty borrow slot available in obligation")]
    ObligationBorrowsFull,
    #[msg("Deposit reserve not found in obligation")]
    DepositNotFound,
    #[msg("Borrow reserve not found in obligation")]
    BorrowNotFound,
    #[msg("Borrow rate decreased unexpectedly")]
    BorrowRateDecreased,
    #[msg("Invalid amount send dby user")]
    InvalidAmount,
    #[msg("Oracle price is stale")]
    PriceStale,
    #[msg("Oracle price is not valid")]
    PriceNotValid,
    #[msg("Oracle confidence interval too wide")]
    PriceConfidenceTooWide,
    #[msg("Obligation is unhealthy")]
    UnhealthyObligation,
    #[msg("Obligation must be refreshed before borrowing")]
    ObligationStale,
    #[msg("Obligation is healthy and cannot be liquidated")]
    ObligationHealthy,
}
