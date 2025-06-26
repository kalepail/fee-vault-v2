use crate::{
    constants::{SCALAR_12, SCALAR_7, SECONDS_PER_YEAR},
    errors::FeeVaultError,
    pool,
    rewards::update_rewards,
    storage,
    validator::require_positive,
};
use soroban_fixed_point_math::{i128, FixedPoint};
use soroban_sdk::{contracttype, panic_with_error, unwrap::UnwrapOptimized, Address, Env};

#[contracttype]
pub struct VaultData {
    /// The timestamp of the last update
    pub last_update_timestamp: u64,
    /// The reserve's last bRate
    pub b_rate: i128,
    /// The total shares issued by the reserve vault
    pub total_shares: i128,
    /// The total bToken deposits owned by the reserve vault depositors. Excludes admin balance.
    pub total_b_tokens: i128,
    /// The admin's bTokens. Excluded from the `total_b_tokens` value.
    pub admin_balance: i128,
}

impl VaultData {
    /// Converts a b_token amount to shares rounding down
    pub fn b_tokens_to_shares_down(&self, amount: i128) -> i128 {
        if self.total_shares == 0 || self.total_b_tokens == 0 {
            return amount;
        }
        amount
            .fixed_mul_floor(self.total_shares, self.total_b_tokens)
            .unwrap_optimized()
    }

    /// Converts a b_token amount to shares rounding up
    pub fn b_tokens_to_shares_up(&self, amount: i128) -> i128 {
        if self.total_shares == 0 || self.total_b_tokens == 0 {
            return amount;
        }
        amount
            .fixed_mul_ceil(self.total_shares, self.total_b_tokens)
            .unwrap_optimized()
    }

    /// Coverts a share amount to a b_token amount rounding down
    pub fn shares_to_b_tokens_down(&self, amount: i128) -> i128 {
        amount
            .fixed_div_floor(self.total_shares, self.total_b_tokens)
            .unwrap_optimized()
    }

    /// Coverts a b_token amount to an underlying token amount rounding down
    pub fn b_tokens_to_underlying_down(&self, amount: i128) -> i128 {
        amount
            .fixed_mul_floor(self.b_rate, SCALAR_12)
            .unwrap_optimized()
    }

    /// Coverts an underlying amount to a b_token amount rounding down
    pub fn underlying_to_b_tokens_down(&self, amount: i128) -> i128 {
        amount
            .fixed_div_floor(self.b_rate, SCALAR_12)
            .unwrap_optimized()
    }

    /// Coverts an underlying amount to a b_token amount rounding up
    pub fn underlying_to_b_tokens_up(&self, amount: i128) -> i128 {
        amount
            .fixed_div_ceil(self.b_rate, SCALAR_12)
            .unwrap_optimized()
    }

    /// Updates the reserve's bRate and accrues fees to the admin in accordance with the portion of interest they earned
    fn update_rate(&mut self, e: &Env, pool: &Address, asset: &Address) {
        let now = e.ledger().timestamp();
        let new_rate = pool::reserve_b_rate(e, &pool, &asset);
        // if the rate didn't increase, admin won't take any fees, so short circuit the math
        // and just apply the b_rate update here
        if new_rate <= self.b_rate {
            self.last_update_timestamp = now;
            self.b_rate = new_rate;
            return;
        }

        let fee = storage::get_fee(e);
        // this can round to zero if new_rate ~= target_b_rate
        // admin_b_tokens calc should round down, to prevent any rounding spam exploits
        let admin_b_tokens: i128 = match fee.rate_type {
            0 => {
                // take rate - admin earns a percentage of the interest accrued
                let admin_take_rate = fee.rate as i128;
                self.total_b_tokens
                    .fixed_mul_floor(new_rate - self.b_rate, SCALAR_12)
                    .unwrap_optimized()
                    .fixed_mul_floor(admin_take_rate, SCALAR_7)
                    .unwrap_optimized()
                    .fixed_div_floor(new_rate, SCALAR_12)
                    .unwrap_optimized()
            }
            1 | 2 => {
                // 1 - capped rate - admin earns a percentage of the interest accrued
                // 2 - fixed rate - admin either earns or supplements the vault to ensure the vault earns the target rate
                //
                // Both rate types calculate the difference in `b_tokens` the vault has vs the target rate. This is done by finding the
                // expected `b_rate` needed to acheived the target rate over the update period, and then determining
                // the `b_tokens` needed to make up the difference between the current `b_rate` and the target `b_rate`.
                //
                // However, capped rates do not get supplemented by the admin, so `admin_b_tokens` can't be negative, while
                // fixed rates can be negative to force the admin to pay the difference into the vault.

                let target_apr = fee.rate as i128;
                let time_elapsed = now - self.last_update_timestamp;

                // Target growth rate for target APR over the time elapsed scaled to 12 decimals
                // -> target_apr is 7 decimals, so we multiply by 100_000 to get 12 decimals (seconds per year and
                //    time elapsed have no decimals)
                let target_growth_rate =
                    (100_000 * target_apr * (time_elapsed as i128)) / SECONDS_PER_YEAR + SCALAR_12;

                let target_b_rate = self
                    .b_rate
                    .fixed_mul_ceil(target_growth_rate, SCALAR_12)
                    .unwrap_optimized();

                // math lib treats floor as rounding away from 0 for negative numbers
                let b_token_diff = self
                    .total_b_tokens
                    .fixed_mul_floor(new_rate - target_b_rate, SCALAR_12)
                    .unwrap_optimized()
                    .fixed_div_floor(new_rate, SCALAR_12)
                    .unwrap_optimized();

                if b_token_diff <= 0 && fee.rate_type == 1 {
                    // capped rate - no fees if the target rate wasn't reached
                    0
                } else {
                    b_token_diff
                }
            }
            // If the fee rate type is malformed, don't accrue any fees for the admin to prevent
            // funds from being locked in the contract. This should never happen.
            _ => 0,
        };

        self.last_update_timestamp = now;
        self.b_rate = new_rate;

        // if no interest was accrued we do not accrue fees
        if admin_b_tokens == 0 {
            return;
        }

        self.total_b_tokens = self.total_b_tokens - admin_b_tokens;
        self.admin_balance = self.admin_balance + admin_b_tokens;
    }
}

/// Get the reserve vault from storage and update the bRate
///
/// ### Arguments
/// * `pool` - The pool address
/// * `asset` - The asset address
///
/// ### Returns
/// * `VaultData` - The updated reserve vault
pub fn get_vault_updated(e: &Env, pool: &Address, asset: &Address) -> VaultData {
    let mut vault = storage::get_vault_data(e);
    vault.update_rate(e, pool, asset);
    vault
}

/// Deposit into the vault. Does not perform the call to the pool to deposit the tokens.
///
/// ### Arguments
/// * `pool` - The pool address
/// * `asset` - The asset address
/// * `user` - The user that deposited the tokens
/// * `amount` - The amount of underlying deposited
///
/// ### Returns
/// * `(i128, i128)` - (The amount of b_tokens minted to the vault, the amount of shares minted to the user)
///
/// ### Panics
/// * If the underlying amount is less than or equal to 0
pub fn deposit(
    e: &Env,
    pool: &Address,
    asset: &Address,
    user: &Address,
    amount: i128,
) -> (i128, i128) {
    let mut vault = get_vault_updated(e, pool, asset);
    let mut user_shares = storage::get_vault_shares(e, user);

    update_rewards(e, vault.total_shares, user, user_shares);

    let b_tokens_amount = vault.underlying_to_b_tokens_down(amount);
    require_positive(e, b_tokens_amount, FeeVaultError::InvalidBTokensMinted);
    let share_amount = vault.b_tokens_to_shares_down(b_tokens_amount);
    require_positive(e, share_amount, FeeVaultError::InvalidSharesMinted);

    vault.total_shares += share_amount;
    vault.total_b_tokens += b_tokens_amount;
    user_shares += share_amount;
    storage::set_vault_data(e, &vault);
    storage::set_vault_shares(e, user, user_shares);
    (b_tokens_amount, share_amount)
}

/// Withdraw from the vault. Does not perform the call to the pool to withdraw the tokens.
///
/// ### Arguments
/// * `pool` - The pool address
/// * `asset` - The user address
/// * `user` - The user withdrawing tokens
/// * `amount` - The amount of underlying amount withdrawn from the vault
///
/// ### Returns
/// * `(i128, i128)` - (The amount of b_tokens burned from the vault, the amount of shares burned from the user)
///
/// ### Panics
/// * If the amount is less than or equal to 0
/// * If the user does not have enough shares or bTokens to withdraw
pub fn withdraw(
    e: &Env,
    pool: &Address,
    asset: &Address,
    user: &Address,
    amount: i128,
) -> (i128, i128) {
    let mut vault = get_vault_updated(e, pool, asset);
    let mut user_shares = storage::get_vault_shares(e, user);

    update_rewards(e, vault.total_shares, user, user_shares);

    let b_tokens_amount = vault.underlying_to_b_tokens_up(amount);
    require_positive(e, b_tokens_amount, FeeVaultError::InvalidBTokensBurnt);
    let share_amount = vault.b_tokens_to_shares_up(b_tokens_amount);
    require_positive(e, share_amount, FeeVaultError::InvalidSharesBurnt);

    if vault.total_shares < share_amount || vault.total_b_tokens < b_tokens_amount {
        panic_with_error!(e, FeeVaultError::InsufficientReserves);
    }

    if share_amount > user_shares {
        panic_with_error!(e, FeeVaultError::BalanceError);
    }
    vault.total_shares -= share_amount;
    vault.total_b_tokens -= b_tokens_amount;

    user_shares -= share_amount;
    storage::set_vault_data(e, &vault);
    storage::set_vault_shares(e, user, user_shares);
    (b_tokens_amount, share_amount)
}

/// Admin deposits tokens into the vault. Does not perform the call to the pool to deposit the tokens.
///
/// ### Arguments
/// * `pool` - The pool address
/// * `asset` - The asset address
/// * `amount` - The amount of tokens to deposit into the vault
///
/// ### Returns
/// * The amount of bTokens added to the admin balance
pub fn admin_deposit(e: &Env, pool: &Address, asset: &Address, amount: i128) -> i128 {
    let mut vault = get_vault_updated(e, pool, asset);

    let b_tokens_amount = vault.underlying_to_b_tokens_down(amount);
    require_positive(e, b_tokens_amount, FeeVaultError::InvalidBTokensMinted);

    vault.admin_balance += b_tokens_amount;

    storage::set_vault_data(e, &vault);
    b_tokens_amount
}

/// Admin withdraws tokens from the vault. Does not perform the call to the pool to withdraw the tokens.
///
/// ### Arguments
/// * `pool` - The pool address
/// * `asset` - The asset address
/// * `amount` - The amount of tokens to withdraw from the vault
///
/// ### Returns
/// * The amount of bTokens burnt from the admin balance
///
/// ### Panics
/// * If the admin balance does not have enough bTokens to withdraw
pub fn admin_withdraw(e: &Env, pool: &Address, asset: &Address, amount: i128) -> i128 {
    let mut vault = get_vault_updated(e, pool, asset);

    let b_tokens_burnt = vault.underlying_to_b_tokens_up(amount);
    require_positive(e, b_tokens_burnt, FeeVaultError::InvalidBTokensBurnt);

    if b_tokens_burnt > vault.admin_balance {
        panic_with_error!(e, FeeVaultError::BalanceError);
    }
    vault.admin_balance -= b_tokens_burnt;

    storage::set_vault_data(e, &vault);
    b_tokens_burnt
}

#[cfg(test)]
mod generic_tests {
    use super::*;
    use crate::testutils::{create_test_fee_vault, mockpool::MockPoolClient, EnvTestUtils};
    use soroban_sdk::{testutils::Address as _, Address};

    #[test]
    fn test_b_tokens_to_shares_down() {
        let mut vault = VaultData {
            b_rate: 1_000_000_000_000,
            last_update_timestamp: 0,
            total_shares: 0,
            total_b_tokens: 0,
            admin_balance: 0,
        };

        // rounds down
        vault.total_shares = 200_0000001;
        vault.total_b_tokens = 100_0000000;
        let b_tokens = vault.b_tokens_to_shares_down(1_0000000);
        assert_eq!(b_tokens, 2_0000000);

        // returns amount if total_shares is 0
        vault.total_shares = 0;
        vault.total_b_tokens = 100_0000000;
        let b_tokens = vault.b_tokens_to_shares_down(1_0000000);
        assert_eq!(b_tokens, 1_0000000);

        // returns amount if total_b_tokens is 0
        vault.total_shares = 200_0000000;
        vault.total_b_tokens = 0;
        let b_tokens = vault.b_tokens_to_shares_down(1_0000000);
        assert_eq!(b_tokens, 1_0000000);
    }

    #[test]
    fn test_b_tokens_to_shares_up() {
        let mut vault = VaultData {
            b_rate: 1_000_000_000_000,
            last_update_timestamp: 0,
            total_shares: 0,
            total_b_tokens: 0,
            admin_balance: 0,
        };

        // rounds up
        vault.total_shares = 200_0000001;
        vault.total_b_tokens = 100_0000000;
        let b_tokens = vault.b_tokens_to_shares_up(1_0000000);
        assert_eq!(b_tokens, 2_0000001);

        // returns amount if total_shares is 0
        vault.total_shares = 0;
        vault.total_b_tokens = 100_0000000;
        let b_tokens = vault.b_tokens_to_shares_up(1_0000000);
        assert_eq!(b_tokens, 1_0000000);

        // returns amount if total_b_tokens is 0
        vault.total_shares = 200_0000000;
        vault.total_b_tokens = 0;
        let b_tokens = vault.b_tokens_to_shares_up(1_0000000);
        assert_eq!(b_tokens, 1_0000000);
    }

    #[test]
    fn test_shares_to_b_tokens_down() {
        let mut vault = VaultData {
            b_rate: 1_000_000_000_000,
            last_update_timestamp: 0,
            total_shares: 0,
            total_b_tokens: 0,
            admin_balance: 0,
        };

        // rounds down
        vault.total_shares = 200_0000001;
        vault.total_b_tokens = 100_0000000;
        let b_tokens = vault.shares_to_b_tokens_down(2_0000000);
        assert_eq!(b_tokens, 0_9999999);

        // returns 0 if total_b_tokens is 0
        vault.total_shares = 200_0000000;
        vault.total_b_tokens = 0;
        let b_tokens = vault.shares_to_b_tokens_down(2_0000000);
        assert_eq!(b_tokens, 0);
    }

    #[test]
    fn test_deposit() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        let init_b_rate = 1_100_000_000_000;
        let mock_client = MockPoolClient::new(&e, &pool);
        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            // Perform a deposit for samwise
            let new_b_rate = 1_110_000_000_000;
            mock_client.set_b_rate(&new_b_rate);
            e.jump(5);

            let b_tokens = 83_3333300;
            let amount = b_tokens
                .fixed_mul_floor(new_b_rate, SCALAR_12)
                .unwrap_optimized();
            let expected_b_token_fees = 0_9009009;
            let expected_share_amount = 100_0901673;
            let (b_tokens_minted, shares_minted) = deposit(&e, &pool, &asset, &samwise, amount);
            assert_eq!(b_tokens_minted, b_tokens);
            assert_eq!(shares_minted, expected_share_amount);

            // Load the updated vault to verify the changes
            let new_vault = storage::get_vault_data(&e);
            assert_eq!(new_vault.total_shares, 1200_0000000 + expected_share_amount);
            assert_eq!(
                new_vault.total_b_tokens,
                1000_0000000 + b_tokens - expected_b_token_fees
            );
            assert_eq!(new_vault.b_rate, new_b_rate);
            assert_eq!(new_vault.admin_balance, expected_b_token_fees);

            let new_balance = storage::get_vault_shares(&e, &samwise);
            assert_eq!(new_balance, expected_share_amount);
        });
    }

    #[test]
    fn test_initial_deposit() {
        let e = Env::default();
        e.mock_all_auths_allowing_non_root_auth();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        let init_b_rate = 1_000_000_000_000;
        let mock_client = MockPoolClient::new(&e, &pool);
        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 0,
                total_shares: 0,
                b_rate: init_b_rate,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            // Perform a deposit for samwise
            let new_b_rate = 1_100_000_000_000;
            mock_client.set_b_rate(&new_b_rate);
            e.jump(5);
            let amount = 100_0000000;
            let expected_b_tokens = amount
                .fixed_div_floor(new_b_rate, SCALAR_12)
                .unwrap_optimized();
            let (b_tokens_minted, shares_minted) = deposit(&e, &pool, &asset, &samwise, amount);

            // Load the updated vault to verify the changes
            let expected_share_amount = expected_b_tokens;
            assert_eq!(b_tokens_minted, expected_b_tokens);
            assert_eq!(shares_minted, expected_share_amount);
            let new_vault = storage::get_vault_data(&e);
            assert_eq!(new_vault.total_shares, expected_share_amount);
            assert_eq!(new_vault.total_b_tokens, b_tokens_minted);
            assert_eq!(new_vault.b_rate, new_b_rate);
            // no fees should accrue against 0 deposits
            assert_eq!(new_vault.admin_balance, 0);

            let new_balance = storage::get_vault_shares(&e, &samwise);
            assert_eq!(new_balance, expected_share_amount);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #106)")]
    fn test_deposit_zero_amount() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);
        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            deposit(&e, &pool, &asset, &samwise, 0);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #106)")]
    fn test_deposit_zero_b_tokens() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            deposit(&e, &pool, &asset, &samwise, 1);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #108)")]
    fn test_deposit_zero_shares() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        e.as_contract(&vault_address, || {
            // Not possible config in practice, but just in case
            let vault_data = VaultData {
                total_b_tokens: 10000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            deposit(&e, &pool, &asset, &samwise, 2);
        });
    }

    #[test]
    fn test_withdraw() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let mut vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            // Perform a withdraw for samwise
            let new_b_rate = 1_110_000_000_000;
            mock_client.set_b_rate(&new_b_rate);
            e.jump(5);

            let b_tokens_to_withdraw = 50_0000000;
            let expected_share_amount = 100_0901674;
            let expected_b_token_fees = 0_9009009;
            storage::set_vault_shares(&e, &samwise, expected_share_amount);

            // update vault to get correct underlying amount. don't set.
            vault_data.b_rate = new_b_rate;
            let withdraw_amount = vault_data.b_tokens_to_underlying_down(b_tokens_to_withdraw);

            let (b_tokens_burnt, shares_burnt) =
                withdraw(&e, &pool, &asset, &samwise, withdraw_amount);

            let new_vault = storage::get_vault_data(&e);
            assert_eq!(b_tokens_burnt, b_tokens_to_withdraw);
            assert_eq!(
                shares_burnt,
                new_vault.b_tokens_to_shares_up(b_tokens_to_withdraw)
            );

            // Load the updated reserve to verify the changes
            assert_eq!(new_vault.total_shares, 1200_0000000 - shares_burnt);
            assert_eq!(
                new_vault.total_b_tokens,
                1000_0000000 - b_tokens_to_withdraw - expected_b_token_fees
            );
            assert_eq!(new_vault.b_rate, new_b_rate);
            assert_eq!(new_vault.admin_balance, expected_b_token_fees);

            let new_balance = storage::get_vault_shares(&e, &samwise);
            assert_eq!(new_balance, expected_share_amount - shares_burnt);
        });
    }

    #[test]
    fn test_withdraw_max() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            storage::set_vault_shares(&e, &samwise, vault_data.total_shares);
            let withdraw_amount = vault_data.b_tokens_to_underlying_down(1000_0000000);

            let (b_tokens_burnt, shares_burnt) =
                withdraw(&e, &pool, &asset, &samwise, withdraw_amount);
            assert_eq!(b_tokens_burnt, 1000_0000000);
            assert_eq!(shares_burnt, 1200_0000000);
            let new_balance = storage::get_vault_shares(&e, &samwise);
            assert_eq!(new_balance, 0);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #107)")]
    fn test_withdraw_zero_amount() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            withdraw(&e, &pool, &asset, &samwise, 0);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #105)")]
    fn test_withdraw_more_b_tokens_than_vault() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            storage::set_vault_shares(&e, &samwise, vault_data.total_shares);
            let withdraw_amount = vault_data.b_tokens_to_underlying_down(1000_0000000);

            withdraw(&e, &pool, &asset, &samwise, withdraw_amount + 1);
        });
    }

    #[test]
    fn test_withdraw_exact_balance() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            let sam_shares = 1000_0000000;
            storage::set_vault_shares(&e, &samwise, sam_shares);
            let sam_b_tokens: i128 =
                vault_data.shares_to_b_tokens_down(storage::get_vault_shares(&e, &samwise));
            let sam_underlying_balance = vault_data.b_tokens_to_underlying_down(sam_b_tokens);

            // Withdraw whole underlying balance as read by the contract
            let (b_tokens_burnt, shares_burnt) =
                withdraw(&e, &pool, &asset, &samwise, sam_underlying_balance);
            assert_eq!(b_tokens_burnt, sam_b_tokens);
            assert_eq!(shares_burnt, sam_shares);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #10)")]
    fn test_withdraw_over_balance() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            storage::set_vault_shares(&e, &samwise, 1000_0000000);
            let sam_b_tokens: i128 = vault_data.shares_to_b_tokens_down(1000_0000000);
            let sam_underlying_balance = vault_data.b_tokens_to_underlying_down(sam_b_tokens);
            // Try to withdraw 1 more than `sam_underlying_balance`
            withdraw(&e, &pool, &asset, &samwise, sam_underlying_balance + 1);
        });
    }

    #[test]
    fn test_admin_deposit() {
        let e = Env::default();
        e.mock_all_auths();

        let init_b_rate = 1_100_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 0, 0_1000000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let starting_admin_balance = 5_0000000;
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: starting_admin_balance,
            };
            storage::set_vault_data(&e, &vault_data);

            // Perform a deposit for the admin
            let new_b_rate = 1_110_000_000_000;
            mock_client.set_b_rate(&new_b_rate);
            e.jump(5);
            let expected_b_token_fees = 0_9009009;
            let b_token_deposit = 10_0000000;
            let amount = b_token_deposit
                .fixed_mul_floor(new_b_rate, SCALAR_12)
                .unwrap_optimized();

            let deposit_result = admin_deposit(&e, &pool, &asset, amount);

            assert_eq!(deposit_result, b_token_deposit);

            // Load the updated reserve to verify the changes
            let new_vault = storage::get_vault_data(&e);
            assert_eq!(new_vault.total_shares, 1200_0000000);
            assert_eq!(
                new_vault.total_b_tokens,
                1000_0000000 - expected_b_token_fees
            );
            assert_eq!(new_vault.b_rate, new_b_rate);
            assert_eq!(
                new_vault.admin_balance,
                starting_admin_balance + expected_b_token_fees + b_token_deposit
            );
            assert_eq!(new_vault.last_update_timestamp, e.ledger().timestamp());
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #106)")]
    fn test_admin_deposit_zero_mint() {
        let e = Env::default();
        e.mock_all_auths();

        let init_b_rate = 1_100_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 0, 0_1000000, Some(init_b_rate));

        e.as_contract(&vault_address, || {
            let starting_admin_balance = 5_0000000;
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: starting_admin_balance,
            };
            storage::set_vault_data(&e, &vault_data);

            admin_deposit(&e, &pool, &asset, 1);
        });
    }

    #[test]
    fn test_admin_withdraw() {
        let e = Env::default();
        e.mock_all_auths();

        let init_b_rate = 1_100_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 0, 0_1000000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let starting_admin_balance = 5_0000000;
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: starting_admin_balance,
            };
            storage::set_vault_data(&e, &vault_data);

            // Perform a deposit for the admin
            let new_b_rate = 1_110_000_000_000;
            mock_client.set_b_rate(&new_b_rate);
            e.jump(5);
            let expected_b_token_fees = 0_9009009;
            let b_token_withdraw = 3_0000000;
            let amount = b_token_withdraw
                .fixed_mul_floor(new_b_rate, SCALAR_12)
                .unwrap_optimized();

            let withdraw_result = admin_withdraw(&e, &pool, &asset, amount);

            assert_eq!(withdraw_result, b_token_withdraw);

            // Load the updated reserve to verify the changes
            let new_vault = storage::get_vault_data(&e);
            assert_eq!(new_vault.total_shares, 1200_0000000);
            assert_eq!(
                new_vault.total_b_tokens,
                1000_0000000 - expected_b_token_fees
            );
            assert_eq!(new_vault.b_rate, new_b_rate);
            assert_eq!(
                new_vault.admin_balance,
                starting_admin_balance + expected_b_token_fees - b_token_withdraw
            );
            assert_eq!(new_vault.last_update_timestamp, e.ledger().timestamp());
        });
    }

    #[test]
    fn test_admin_withdraw_all() {
        let e = Env::default();
        e.mock_all_auths();

        let init_b_rate = 1_100_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 0, 0_1000000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let starting_admin_balance = 5_0000000;
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: starting_admin_balance,
            };
            storage::set_vault_data(&e, &vault_data);

            // Perform a withdraw for the admin
            let new_b_rate = 1_110_000_000_000;
            mock_client.set_b_rate(&new_b_rate);
            e.jump(5);
            let expected_b_token_fees = 0_9009009;
            let b_token_withdraw = starting_admin_balance + expected_b_token_fees;
            let amount = b_token_withdraw
                .fixed_mul_floor(new_b_rate, SCALAR_12)
                .unwrap_optimized();

            let withdraw_result = admin_withdraw(&e, &pool, &asset, amount);

            assert_eq!(withdraw_result, b_token_withdraw);

            // Load the updated reserve to verify the changes
            let new_vault = storage::get_vault_data(&e);
            assert_eq!(new_vault.total_shares, 1200_0000000);
            assert_eq!(
                new_vault.total_b_tokens,
                1000_0000000 - expected_b_token_fees
            );
            assert_eq!(new_vault.b_rate, new_b_rate);
            assert_eq!(new_vault.admin_balance, 0);
            assert_eq!(new_vault.last_update_timestamp, e.ledger().timestamp());
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #10)")]
    fn test_admin_withdraw_balance_error() {
        let e = Env::default();
        e.mock_all_auths();

        let init_b_rate = 1_100_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 0, 0_1000000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let starting_admin_balance = 5_0000000;
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: starting_admin_balance,
            };
            storage::set_vault_data(&e, &vault_data);

            // Perform a withdraw for the admin
            let new_b_rate = 1_110_000_000_000;
            mock_client.set_b_rate(&new_b_rate);
            e.jump(5);
            let expected_b_token_fees = 0_9009009;
            let b_token_withdraw = starting_admin_balance + expected_b_token_fees + 1;
            let amount = b_token_withdraw
                .fixed_mul_floor(new_b_rate, SCALAR_12)
                .unwrap_optimized();

            admin_withdraw(&e, &pool, &asset, amount);
        });
    }

    #[test]
    fn test_admin_withdraw_zero_shares() {
        let e = Env::default();
        e.mock_all_auths();

        let init_b_rate = 1_100_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 0, 0_1000000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let starting_admin_balance = 5_0000000;
            let vault_data = VaultData {
                total_b_tokens: 0,
                total_shares: 0,
                b_rate: init_b_rate,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: starting_admin_balance,
            };
            storage::set_vault_data(&e, &vault_data);

            // Perform a withdraw for the admin
            // Even if b_rate doubles, since there are no b_tokens deposited, no more fees should've been accrued
            let new_b_rate = 2_200_000_000_000;
            mock_client.set_b_rate(&new_b_rate);
            e.jump(5);
            let amount = starting_admin_balance
                .fixed_mul_floor(new_b_rate, SCALAR_12)
                .unwrap_optimized();

            let withdraw_result = admin_withdraw(&e, &pool, &asset, amount);

            assert_eq!(withdraw_result, starting_admin_balance);

            // Load the updated reserve to verify the changes
            let new_vault = storage::get_vault_data(&e);
            assert_eq!(new_vault.total_shares, 0);
            assert_eq!(new_vault.total_b_tokens, 0);
            assert_eq!(new_vault.b_rate, new_b_rate);
            assert_eq!(new_vault.admin_balance, 0);
            assert_eq!(new_vault.last_update_timestamp, e.ledger().timestamp());
        });
    }
}

#[cfg(test)]
mod take_rate_tests {
    use super::*;
    use crate::testutils::{create_test_fee_vault, mockpool::MockPoolClient, EnvTestUtils};
    use soroban_sdk::{testutils::Address as _, Address};

    #[test]
    fn test_update_rate() {
        let e = Env::default();
        e.mock_all_auths();

        let init_b_rate = 1_100_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 0, 0_2000000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let mut vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                last_update_timestamp: e.ledger().timestamp(),
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                admin_balance: 0,
            };

            // update b_rate to 1.2
            let expected_accrued_fee = 16_6666666;
            mock_client.set_b_rate(&120_000_0000_000);
            e.jump(5);
            vault_data.update_rate(&e, &pool, &asset);

            assert_eq!(vault_data.admin_balance, expected_accrued_fee);
            assert_eq!(vault_data.total_shares, 1200_000_0000);
            assert_eq!(vault_data.total_b_tokens, 1000_0000000 - 16_6666666);

            // update b_rate to 1.5
            let expected_accrued_fee_2 = 39_333_3333;
            mock_client.set_b_rate(&150_000_0000_000);
            e.jump(5);

            vault_data.update_rate(&e, &pool, &asset);

            assert_eq!(
                vault_data.admin_balance,
                expected_accrued_fee + expected_accrued_fee_2
            );
            assert_eq!(vault_data.total_shares, 1200_000_0000);
            assert_eq!(
                vault_data.total_b_tokens,
                1000_0000000 - 16_6666666 - 39_333_3333
            );
        });
    }

    #[test]
    fn test_update_rate_2() {
        let e = Env::default();
        e.mock_all_auths();

        let init_b_rate = 1_000_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 0, 0_2000000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let mut vault_data = VaultData {
                total_b_tokens: 500_000_0000000,
                total_shares: 500_000_0000000,
                b_rate: init_b_rate,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };

            let expected_accrued_fee = 1050_1384599;

            let new_b_rate = 1_000_0000000 * SCALAR_12 / 989_4986154;
            mock_client.set_b_rate(&new_b_rate);
            e.jump(5);
            vault_data.update_rate(&e, &pool, &asset);
            let deposit_b_tokens = 989_4986154;
            let shares = vault_data.b_tokens_to_shares_down(deposit_b_tokens);
            vault_data.total_b_tokens += deposit_b_tokens;
            vault_data.total_shares += shares;
            assert_eq!(
                vault_data.total_b_tokens,
                500_000_0000000 + 989_4986154 - expected_accrued_fee
            );
            assert_eq!(vault_data.total_shares, 500_000_0000000 + 991_5812105);

            assert_eq!(vault_data.b_rate, 1_010_612_834_052);

            assert_eq!(vault_data.admin_balance, expected_accrued_fee);
        });
    }

    #[test]
    fn test_update_rate_no_change() {
        let e = Env::default();
        e.mock_all_auths();

        let init_b_rate = 1_100_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 0, 0_1000000, Some(init_b_rate));

        e.as_contract(&vault_address, || {
            let now = e.ledger().timestamp();
            let mut vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                last_update_timestamp: now,
                admin_balance: 12_0000000,
            };

            vault_data.update_rate(&e, &pool, &asset);
            // assert nothing changes
            assert_eq!(vault_data.admin_balance, vault_data.admin_balance);
            assert_eq!(vault_data.total_shares, vault_data.total_shares);
            assert_eq!(vault_data.total_b_tokens, vault_data.total_b_tokens);
            assert_eq!(vault_data.b_rate, vault_data.b_rate);
            assert_eq!(vault_data.last_update_timestamp, now);
        });
    }

    #[test]
    fn test_update_rate_different_timestamp_same_brate() {
        let e = Env::default();
        e.mock_all_auths();

        let init_b_rate = 1_100_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 0, 0_1000000, Some(init_b_rate));

        e.as_contract(&vault_address, || {
            let now = e.ledger().timestamp();
            let mut vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                last_update_timestamp: now,
                admin_balance: 12_0000000,
            };

            e.jump_time(100);

            vault_data.update_rate(&e, &pool, &asset);
            // assert nothing changes
            assert_eq!(vault_data.admin_balance, vault_data.admin_balance);
            assert_eq!(vault_data.total_shares, vault_data.total_shares);
            assert_eq!(vault_data.total_b_tokens, vault_data.total_b_tokens);
            assert_eq!(vault_data.b_rate, vault_data.b_rate);
            // Assert the timestamp still gets updated
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());
        });
    }

    #[test]
    fn test_update_rate_negative_value() {
        let e = Env::default();
        e.mock_all_auths();
        e.set_default_info();

        let init_b_rate = 1_100_000_000_000;

        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 0, 0_1000000, Some(init_b_rate));
        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let mut vault_data = VaultData {
                total_b_tokens: 100_0000000,
                last_update_timestamp: e.ledger().timestamp(),
                total_shares: 100_0000000,
                b_rate: init_b_rate,
                admin_balance: 0,
            };

            // negative rate
            let new_b_rate: i128 = 1_050_000_000_000;
            mock_client.set_b_rate(&new_b_rate);
            vault_data.update_rate(&e, &pool, &asset);

            // Assert b_rate change is reflected
            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.admin_balance, 0);
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());

            // negative rate - time change - does not deduct already accrued fees
            let new_b_rate: i128 = 1_010_000_000_000;
            vault_data.admin_balance = 100_0000;
            mock_client.set_b_rate(&new_b_rate);
            e.jump(5);
            vault_data.update_rate(&e, &pool, &asset);

            // Assert nothing changes apart
            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.admin_balance, 100_0000);
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());
        });
    }

    #[test]
    fn test_update_rate_rounds_down() {
        let e = Env::default();
        e.mock_all_auths();
        e.set_default_info();

        let init_b_rate = 1_000_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 0, 0_1000000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let init_timestamp = e.ledger().timestamp();
            let mut vault_data = VaultData {
                total_b_tokens: 100_0000000,
                last_update_timestamp: init_timestamp,
                total_shares: 100_0000000,
                b_rate: init_b_rate,
                admin_balance: 0,
            };

            // 2% rate over 5s - too small for vault to capture any interest
            // for for the admin with a 1% take rate
            let new_b_rate: i128 = 1_000_000_003_171;
            mock_client.set_b_rate(&new_b_rate);
            e.jump_time(5);
            vault_data.update_rate(&e, &pool, &asset);

            // Assert b_rate change is reflected but rounds fees to zero if less than a stroop amount
            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.admin_balance, 0);
            assert_eq!(vault_data.last_update_timestamp, init_timestamp + 5);

            // Assert with enough b_tokens to capture interest still rounds down
            // reset vault
            vault_data.total_b_tokens = 1000_0000000;
            vault_data.b_rate = init_b_rate;
            vault_data.admin_balance = 0;
            vault_data.last_update_timestamp = init_timestamp;

            // new_b_rate already applied to pool
            vault_data.update_rate(&e, &pool, &asset);

            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.admin_balance, 2); // actual is 0_0000002_9
            assert_eq!(vault_data.last_update_timestamp, init_timestamp + 5);
        });
    }
}

#[cfg(test)]
mod apr_capped_tests {
    use super::*;
    use crate::{
        storage::Fee,
        testutils::{
            assert_approx_eq_abs, assert_approx_eq_rel, create_test_fee_vault,
            mockpool::MockPoolClient, EnvTestUtils,
        },
    };
    use soroban_sdk::{testutils::Address as _, Address};

    fn update_b_rate_and_time(
        e: &Env,
        mock_pool_client: &MockPoolClient,
        new_b_rate: i128,
        jump_seconds: u64,
    ) {
        mock_pool_client.set_b_rate(&new_b_rate);
        e.jump_time(jump_seconds);
    }

    #[test]
    fn test_update_rate() {
        let e = Env::default();
        e.mock_all_auths();
        e.set_default_info();

        let init_b_rate = 1_000_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 1, 0_0500000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let mut vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                last_update_timestamp: e.ledger().timestamp(),
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                admin_balance: 0,
            };

            let new_b_rate = 1_050_000_000_000;
            let underlying_value_before =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);
            // Update b_rate to 1.05 over 3 months
            update_b_rate_and_time(&e, &mock_client, new_b_rate, (SECONDS_PER_YEAR as u64) / 4);

            vault_data.update_rate(&e, &pool, &asset);
            let expected_fees = 357142857;

            // We'd expect user's underlying value to have increased by approx. (5/4)%, as the cap is reached
            let underlying_value_after =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);

            assert_approx_eq_rel(
                underlying_value_after,
                underlying_value_before + underlying_value_before * 125 / 10_000,
                0_000001,
            );

            // The 1.25% growth was returned to the users, so we'd expect the admin's accrued fees to be the rest 3.75% of the initial value
            let admin_balance_value =
                vault_data.b_tokens_to_underlying_down(vault_data.admin_balance);
            assert_approx_eq_rel(
                admin_balance_value,
                underlying_value_before * 375 / 10_000,
                0_0000001,
            );

            assert_eq!(vault_data.admin_balance, expected_fees);
            assert_eq!(vault_data.total_shares, 1200_000_0000);
            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());
            assert_eq!(vault_data.total_b_tokens, 1000_0000000 - expected_fees);

            // Update b_rate to 1.06 over 6 months
            let final_b_rate = 1_060_000_000_000;
            update_b_rate_and_time(
                &e,
                &mock_client,
                final_b_rate,
                (SECONDS_PER_YEAR as u64) / 2,
            );
            vault_data.update_rate(&e, &pool, &asset);

            // The target APR wasn't reached, so we expect that the whole interest is distributed to the users, with no fee acrual
            assert_eq!(vault_data.admin_balance, expected_fees);
            assert_eq!(vault_data.total_shares, 1200_000_0000);
            assert_eq!(vault_data.b_rate, final_b_rate);
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());
            assert_eq!(vault_data.total_b_tokens, 1000_0000000 - expected_fees);

            // The user's should still get some value
            let final_underlying_value =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);

            // Approx 0.009% increase. All the value should end up to the users
            let increase_pct = final_b_rate * SCALAR_12 / new_b_rate;
            assert_eq!(
                final_underlying_value,
                underlying_value_after * increase_pct / SCALAR_12
            );

            // Exactly 5% increase over the next year
            let b_rate_after_1_year = 1_113_000_000_000;
            update_b_rate_and_time(
                &e,
                &mock_client,
                b_rate_after_1_year,
                SECONDS_PER_YEAR as u64,
            );

            vault_data.update_rate(&e, &pool, &asset);

            // Still no fees accrued for the admin
            assert_eq!(vault_data.admin_balance, expected_fees);
            // The user's value should've increased by 5% exactly. Accounting for rounding errors
            assert_approx_eq_rel(
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens),
                final_underlying_value * 105 / 100,
                0_0000001,
            );

            assert_eq!(vault_data.b_rate, b_rate_after_1_year);
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());
            assert_eq!(vault_data.total_b_tokens, 1000_0000000 - expected_fees);
            assert_eq!(vault_data.total_shares, 1200_000_0000);
        });
    }

    #[test]
    fn test_update_rate_2() {
        let e = Env::default();
        e.mock_all_auths();
        e.set_default_info();

        let init_b_rate = 1_000_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 1, 0_0600000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let mut vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                last_update_timestamp: e.ledger().timestamp(),
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                admin_balance: 0,
            };

            // Assume no interest accrual for 1 month
            update_b_rate_and_time(
                &e,
                &mock_client,
                init_b_rate,
                (SECONDS_PER_YEAR as u64) / 12,
            );
            vault_data.update_rate(&e, &pool, &asset);

            // Assert nothing changes apart from the timestamp
            assert_eq!(vault_data.b_rate, init_b_rate);
            assert_eq!(vault_data.admin_balance, 0);
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());

            // 1% yield over the next 2 months - exactly 6% yearly
            let new_b_rate = 1_010_000_000_000;

            let pre_update_underlying_value =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);

            update_b_rate_and_time(&e, &mock_client, new_b_rate, (SECONDS_PER_YEAR as u64) / 6);

            vault_data.update_rate(&e, &pool, &asset);

            let post_update_underlying_value =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);
            // we expect that post_update_underlying_value = 1.01 * pre_update_underlying_value
            assert_eq!(
                post_update_underlying_value,
                101 * pre_update_underlying_value / 100
            );
            // The admin still shouldn't have accrued any fees
            assert_eq!(vault_data.admin_balance, 0);
            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());

            // 3% yield over the next 3 months, 12% yearly, so the admin should accrue some fees
            let final_b_rate = 1_040_300_000_000;
            update_b_rate_and_time(
                &e,
                &mock_client,
                final_b_rate,
                (SECONDS_PER_YEAR as u64) / 4,
            );
            vault_data.update_rate(&e, &pool, &asset);

            let final_underlying_value =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);

            // We expect that the underlying value now is (6/4)%=1.5% higher than the previous value
            assert_eq!(
                final_underlying_value,
                post_update_underlying_value * 1015 / 1000
            );
            assert_eq!(vault_data.b_rate, final_b_rate);
            assert_eq!(vault_data.total_shares, 1200_0000000);
            assert_ne!(vault_data.admin_balance, 0);
            assert_eq!(
                vault_data.total_b_tokens,
                1000_0000000 - vault_data.admin_balance
            );

            // Since the growth was 3%, 1.5% should be the user's yield and the rest 1.5% the accrued fees
            let expected_admin_balance = final_underlying_value - post_update_underlying_value;
            let admin_balance_value =
                vault_data.b_tokens_to_underlying_down(vault_data.admin_balance);
            // there may be a small rounding error
            assert_approx_eq_rel(admin_balance_value, expected_admin_balance, 0_0000001);
        });
    }

    #[test]
    fn test_update_rate_capped_rounds_down() {
        let e = Env::default();
        e.mock_all_auths();
        e.set_default_info();

        let init_b_rate = 1_000_000_000_000;

        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 1, 0_0100000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let init_timestamp = e.ledger().timestamp();
            let mut vault_data = VaultData {
                total_b_tokens: 10_0000000,
                last_update_timestamp: init_timestamp,
                total_shares: 10_0000000,
                b_rate: init_b_rate,
                admin_balance: 0,
            };

            // 2% rate over 5s - too small for vault to capture any interest
            // for the admin on the 1% capped rate with 10 b_tokens
            let new_b_rate: i128 = 1_000_000_003_171;
            update_b_rate_and_time(&e, &mock_client, new_b_rate, 5);
            vault_data.update_rate(&e, &pool, &asset);

            // Assert b_rate change is reflected but rounds fees to zero if less than a stroop amount
            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.admin_balance, 0);
            assert_eq!(vault_data.total_b_tokens, 10_0000000);
            assert_eq!(vault_data.total_shares, 10_0000000);
            assert_eq!(vault_data.last_update_timestamp, init_timestamp + 5);

            // Assert with enough b_tokens to capture interest still rounds down
            // reset vault
            vault_data.total_b_tokens = 150_0000000;
            vault_data.b_rate = init_b_rate;
            vault_data.admin_balance = 0;
            vault_data.last_update_timestamp = init_timestamp;

            // new_b_rate already applied to pool
            vault_data.update_rate(&e, &pool, &asset);

            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.admin_balance, 1);
            assert_eq!(vault_data.total_b_tokens, 150_0000000 - 1);
            assert_eq!(vault_data.total_shares, 10_0000000);
            assert_eq!(vault_data.last_update_timestamp, init_timestamp + 5);
        });
    }

    #[test]
    fn update_apr_cap() {
        let e = Env::default();
        e.mock_all_auths();
        e.set_default_info();

        let init_b_rate = 1_000_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 1, 0_1000000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let mut vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                last_update_timestamp: e.ledger().timestamp(),
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                admin_balance: 0,
            };

            // Assume 5% APR over 6 months
            let b_rate = 1_050_000_000_000;
            let pre_update_underlying_value =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);
            update_b_rate_and_time(&e, &mock_client, b_rate, (SECONDS_PER_YEAR as u64) / 2);
            vault_data.update_rate(&e, &pool, &asset);
            let post_update_underlying_value =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);

            // no admin_balance, as the APR is equal to the cap
            assert_eq!(
                post_update_underlying_value,
                105 * pre_update_underlying_value / 100
            );
            assert_eq!(vault_data.b_rate, b_rate);
            assert_eq!(vault_data.admin_balance, 0);
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());

            // The admin decides to update the apr_cap to 5%, as 10% didn't yield any interest to the admin
            storage::set_fee(
                &e,
                Fee {
                    rate_type: 1,
                    rate: 0_0500000,
                },
            );

            // Assume 4% APR increase over the the next 6 months, 8% yearly
            let new_b_rate = 1_092_000_000_000;

            update_b_rate_and_time(&e, &mock_client, new_b_rate, (SECONDS_PER_YEAR as u64) / 2);
            vault_data.update_rate(&e, &pool, &asset);

            let final_underlying_value =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);

            // The target APR is reached, so the users should get an increase of 2.5%
            assert_eq!(
                final_underlying_value,
                post_update_underlying_value * 1025 / 1000
            );
            // The rest 1.5% should be the admin's accrued fees
            let expected_fees = post_update_underlying_value * 15 / 1000;
            let admin_balance_value =
                vault_data.b_tokens_to_underlying_down(vault_data.admin_balance);
            assert_approx_eq_rel(admin_balance_value, expected_fees, 0_0000001);

            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.total_shares, 1200_0000000);
            assert_ne!(vault_data.admin_balance, 0);
            assert_eq!(
                vault_data.total_b_tokens,
                1000_0000000 - vault_data.admin_balance
            );
        });
    }

    #[test]
    fn change_fee_mode() {
        let e = Env::default();
        e.mock_all_auths();
        e.set_default_info();

        let init_b_rate = 1_000_000_000_000;

        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 1, 0_0800000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let mut vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                last_update_timestamp: e.ledger().timestamp(),
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                admin_balance: 0,
            };

            // Assume 10% APR over 12 months
            let b_rate = 1_100_000_000_000;
            let pre_update_underlying_value =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);
            update_b_rate_and_time(&e, &mock_client, b_rate, SECONDS_PER_YEAR as u64);
            vault_data.update_rate(&e, &pool, &asset);
            let post_update_underlying_value =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);
            let admin_balance_value =
                vault_data.b_tokens_to_underlying_down(vault_data.admin_balance);
            // no admin_balance, as the APR is equal to the cap
            assert_eq!(
                post_update_underlying_value,
                108 * pre_update_underlying_value / 100
            );
            // The rest 2% is the fees(there could be a small rounding error)
            assert_approx_eq_rel(
                admin_balance_value,
                2 * pre_update_underlying_value / 100,
                0_0000001,
            );
            assert_eq!(vault_data.b_rate, b_rate);
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());

            // Update the fee mode to take_rate with 20% take rate
            storage::set_fee(
                &e,
                Fee {
                    rate_type: 0,
                    rate: 200_0000,
                },
            );

            let new_b_rate = 1_200_000_000_000;

            update_b_rate_and_time(&e, &mock_client, new_b_rate, SECONDS_PER_YEAR as u64);
            vault_data.update_rate(&e, &pool, &asset);

            // 163636363 accrued fees from this accrual + the pre-existing fees
            let expected_accrued_fee = 34_5454544;

            assert_eq!(vault_data.admin_balance, expected_accrued_fee);
            assert_eq!(vault_data.total_shares, 1200_000_0000);
            assert_eq!(
                vault_data.total_b_tokens,
                1000_0000000 - expected_accrued_fee
            );
            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());
        });
    }

    #[test]
    fn test_update_rate_no_change() {
        let e = Env::default();
        e.mock_all_auths();

        let init_b_rate = 1_100_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 1, 0_0500000, Some(init_b_rate));

        e.as_contract(&vault_address, || {
            let now = e.ledger().timestamp();
            let mut vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                last_update_timestamp: now,
                admin_balance: 12_0000000,
            };

            vault_data.update_rate(&e, &pool, &asset);
            // assert nothing changes
            assert_eq!(vault_data.admin_balance, vault_data.admin_balance);
            assert_eq!(vault_data.total_shares, vault_data.total_shares);
            assert_eq!(vault_data.total_b_tokens, vault_data.total_b_tokens);
            assert_eq!(vault_data.b_rate, vault_data.b_rate);
            assert_eq!(vault_data.last_update_timestamp, now);
        });
    }

    #[test]
    fn test_update_rate_different_timestamp_same_brate() {
        let e = Env::default();
        e.mock_all_auths();

        let init_b_rate = 1_100_000_000_000;

        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 1, 0_0500000, Some(init_b_rate));

        e.as_contract(&vault_address, || {
            let now = e.ledger().timestamp();
            let mut vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                last_update_timestamp: now,
                admin_balance: 12_0000000,
            };

            e.jump_time(100);

            vault_data.update_rate(&e, &pool, &asset);
            // assert nothing changes
            assert_eq!(vault_data.admin_balance, vault_data.admin_balance);
            assert_eq!(vault_data.total_shares, vault_data.total_shares);
            assert_eq!(vault_data.total_b_tokens, vault_data.total_b_tokens);
            assert_eq!(vault_data.b_rate, vault_data.b_rate);
            // assert the timestamp still gets updated
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());
        });
    }

    #[test]
    fn test_update_rate_below_target() {
        let e = Env::default();
        e.mock_all_auths();
        e.set_default_info();

        let init_admin_balance = 10_0000000;
        let init_b_rate = 1_000_000_000_000;
        let init_b_supply = 1000_0000000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 1, 0_0500000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let mut vault_data = VaultData {
                total_b_tokens: init_b_supply,
                last_update_timestamp: e.ledger().timestamp(),
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                admin_balance: init_admin_balance,
            };

            let underlying_value_before =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);

            // approx 3.65% APR over 1 day
            let new_b_rate = 1_000_100_000_000;
            update_b_rate_and_time(&e, &mock_client, new_b_rate, 86400);

            vault_data.update_rate(&e, &pool, &asset);

            let underlying_value_after =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);
            // 0.000_100_000 = 0.0365 * 1 / 365
            assert_approx_eq_abs(
                underlying_value_after,
                underlying_value_before + underlying_value_before * 100_000 / 1_000_000_000,
                10,
            );

            // no b_token fees applied when below target
            assert_eq!(vault_data.admin_balance, init_admin_balance);
            assert_eq!(vault_data.total_shares, 1200_000_0000);
            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());
            assert_eq!(vault_data.total_b_tokens, init_b_supply);
        });
    }
}

#[cfg(test)]
mod fixed_rate_tests {
    use super::*;
    use crate::testutils::{
        assert_approx_eq_abs, create_test_fee_vault, mockpool::MockPoolClient, EnvTestUtils,
    };
    use soroban_sdk::{testutils::Address as _, Address};

    fn update_b_rate_and_time(
        e: &Env,
        mock_pool_client: &MockPoolClient,
        new_b_rate: i128,
        jump_seconds: u64,
    ) {
        mock_pool_client.set_b_rate(&new_b_rate);
        e.jump_time(jump_seconds);
    }

    #[test]
    fn test_update_rate_over_target() {
        let e = Env::default();
        e.mock_all_auths();
        e.set_default_info();

        let init_admin_balance = 10_0000000;
        let init_b_rate = 1_000_000_000_000;
        let init_b_supply = 1000_0000000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 2, 0_0500000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let mut vault_data = VaultData {
                total_b_tokens: init_b_supply,
                last_update_timestamp: e.ledger().timestamp(),
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                admin_balance: init_admin_balance,
            };

            let underlying_value_before =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);

            // approx 10.95% APR over 1 day
            let new_b_rate = 1_000_300_000_000;
            update_b_rate_and_time(&e, &mock_client, new_b_rate, 86400);

            vault_data.update_rate(&e, &pool, &asset);

            let underlying_value_after =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);
            // 0.000_136_986 = 0.05 * 1 / 365
            assert_approx_eq_abs(
                underlying_value_after,
                underlying_value_before + underlying_value_before * 136_986 / 1_000_000_000,
                10,
            );

            let admin_balance_delta = vault_data.admin_balance - init_admin_balance;
            let underlying_admin_delta =
                vault_data.b_tokens_to_underlying_down(admin_balance_delta);
            // 0.000_163_013 = 0.0595 * 1 / 365
            assert_approx_eq_abs(
                underlying_admin_delta,
                underlying_value_before * 163_013 / 1_000_000_000,
                10,
            );

            assert_eq!(
                vault_data.admin_balance,
                init_admin_balance + admin_balance_delta
            );
            assert_eq!(vault_data.total_shares, 1200_000_0000);
            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());
            assert_eq!(
                vault_data.total_b_tokens,
                init_b_supply - admin_balance_delta
            );
        });
    }

    #[test]
    fn test_update_rate_below_target() {
        let e = Env::default();
        e.mock_all_auths();
        e.set_default_info();

        let init_admin_balance = 10_0000000;
        let init_b_rate = 1_000_000_000_000;
        let init_b_supply = 1000_0000000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 2, 0_0500000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let mut vault_data = VaultData {
                total_b_tokens: init_b_supply,
                last_update_timestamp: e.ledger().timestamp(),
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                admin_balance: init_admin_balance,
            };

            let underlying_value_before =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);

            // approx 3.65% APR over 1 day
            let new_b_rate = 1_000_100_000_000;
            update_b_rate_and_time(&e, &mock_client, new_b_rate, 86400);

            vault_data.update_rate(&e, &pool, &asset);

            let underlying_value_after =
                vault_data.b_tokens_to_underlying_down(vault_data.total_b_tokens);
            // 0.000_136_986 = 0.05 * 1 / 365
            assert_approx_eq_abs(
                underlying_value_after,
                underlying_value_before + underlying_value_before * 136_986 / 1_000_000_000,
                10,
            );

            let admin_balance_delta = vault_data.admin_balance - init_admin_balance;
            let underlying_admin_delta =
                vault_data.b_tokens_to_underlying_down(admin_balance_delta);
            // 0.000_036_986 = 0.0135 * 1 / 365
            assert_approx_eq_abs(
                underlying_admin_delta,
                -1 * underlying_value_before * 36_986 / 1_000_000_000,
                10,
            );

            assert_eq!(
                vault_data.admin_balance,
                init_admin_balance + admin_balance_delta
            );
            assert_eq!(vault_data.total_shares, 1200_000_0000);
            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());
            assert_eq!(
                vault_data.total_b_tokens,
                init_b_supply - admin_balance_delta
            );
        });
    }

    #[test]
    fn test_update_rate_over_target_rounds_down() {
        let e = Env::default();
        e.mock_all_auths();
        e.set_default_info();

        let init_b_rate = 1_000_000_000_000;

        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 2, 0_0100000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let init_timestamp = e.ledger().timestamp();
            let mut vault_data = VaultData {
                total_b_tokens: 10_0000000,
                last_update_timestamp: init_timestamp,
                total_shares: 10_0000000,
                b_rate: init_b_rate,
                admin_balance: 0,
            };

            // 2% rate over 5s - too small for vault to capture any interest
            // for the admin on the 1% capped rate with 10 b_tokens
            let new_b_rate: i128 = 1_000_000_003_171;
            update_b_rate_and_time(&e, &mock_client, new_b_rate, 5);
            vault_data.update_rate(&e, &pool, &asset);

            // Assert b_rate change is reflected but rounds fees to zero if less than a stroop amount
            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.admin_balance, 0);
            assert_eq!(vault_data.total_b_tokens, 10_0000000);
            assert_eq!(vault_data.total_shares, 10_0000000);
            assert_eq!(vault_data.last_update_timestamp, init_timestamp + 5);

            // Assert with enough b_tokens to capture interest still rounds down
            // reset vault
            vault_data.total_b_tokens = 150_0000000;
            vault_data.b_rate = init_b_rate;
            vault_data.admin_balance = 0;
            vault_data.last_update_timestamp = init_timestamp;

            // new_b_rate already applied to pool
            vault_data.update_rate(&e, &pool, &asset);

            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.admin_balance, 1);
            assert_eq!(vault_data.total_b_tokens, 150_0000000 - 1);
            assert_eq!(vault_data.total_shares, 10_0000000);
            assert_eq!(vault_data.last_update_timestamp, init_timestamp + 5);
        });
    }

    #[test]
    fn test_update_rate_below_target_rounds_down() {
        let e = Env::default();
        e.mock_all_auths();
        e.set_default_info();

        let init_b_rate = 1_000_000_000_000;

        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) =
            create_test_fee_vault(&e, &bombadil, 2, 0_0300000, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let init_timestamp = e.ledger().timestamp();
            let mut vault_data = VaultData {
                total_b_tokens: 10_0000000,
                last_update_timestamp: init_timestamp,
                total_shares: 10_0000000,
                b_rate: init_b_rate,
                admin_balance: 0,
            };

            // 2% rate over 5s - required supplemental b_tokens below 1 stroop
            let new_b_rate: i128 = 1_000_000_003_171;
            update_b_rate_and_time(&e, &mock_client, new_b_rate, 5);
            vault_data.update_rate(&e, &pool, &asset);

            // Asset admin balance is rounding away from zero
            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.admin_balance, -1);
            assert_eq!(vault_data.total_b_tokens, 10_0000000 + 1);
            assert_eq!(vault_data.total_shares, 10_0000000);
            assert_eq!(vault_data.last_update_timestamp, init_timestamp + 5);

            // Assert with more than 1 stroop of supplemental b_tokens still rounds down
            // reset vault
            vault_data.total_b_tokens = 100_0000000;
            vault_data.b_rate = init_b_rate;
            vault_data.admin_balance = 0;
            vault_data.last_update_timestamp = init_timestamp;

            // new_b_rate already applied to pool
            vault_data.update_rate(&e, &pool, &asset);

            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.admin_balance, -2);
            assert_eq!(vault_data.last_update_timestamp, init_timestamp + 5);
        });
    }
}
