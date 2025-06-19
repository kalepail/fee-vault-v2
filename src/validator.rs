/**
 * Validation functions for the fee vault.
 *
 * Functions in this module must panic if the valid conditions are not met.
 */
use soroban_sdk::{panic_with_error, Env};

use crate::{errors::FeeVaultError, storage::Fee};

/// Require that an incoming amount is positive
///
/// ### Arguments
/// * `amount` - The amount to check
/// * `err` - The error to panic with if the amount is negative or zero
///
/// ### Panics
/// If the number is negative or zero
pub fn require_positive(e: &Env, amount: i128, err: FeeVaultError) {
    if amount <= 0 {
        panic_with_error!(e, err);
    }
}

/// Require that a the rate_type and rate are a valid fee configuration
///
/// ### Arguments
/// * `fee` - The fee configuration to check
pub fn require_valid_fee(e: &Env, fee: &Fee) {
    if fee.rate > 1_000_0000 {
        panic_with_error!(&e, FeeVaultError::InvalidFeeRate);
    }

    if fee.rate_type > 2 {
        panic_with_error!(&e, FeeVaultError::InvalidFeeRateType);
    }
}
