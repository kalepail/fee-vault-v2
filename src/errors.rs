use soroban_sdk::contracterror;

/// The error codes for the contract.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum FeeVaultError {
    // Default errors to align with built-in contract
    BalanceError = 10,

    ReserveNotFound = 100,
    ReserveAlreadyExists = 101,
    InvalidAmount = 102,
    InsufficientAccruedFees = 103,
    InvalidFeeRate = 104,
    InsufficientReserves = 105,
    InvalidBTokensMinted = 106,
    InvalidBTokensBurnt = 107,
    InvalidSharesMinted = 108,
    InvalidFeeRateType = 109,
    NoRewardsConfigured = 110,
    InvalidRewardConfig = 111,
    InvalidSharesBurnt = 112,
}
