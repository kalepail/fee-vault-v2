use soroban_fixed_point_math::FixedPoint;
use soroban_sdk::{panic_with_error, token::TokenClient, unwrap::UnwrapOptimized, Address, Env};

use crate::{
    constants::SCALAR_7,
    errors::FeeVaultError,
    storage::{self, RewardData, UserRewards},
};

/// Update the rewards index for the user and pool. Must be invoked if any changes are made to the total shares
/// or a user's shares in the vault.
///
/// ### Arguments
/// * `total_shares` - The total number of shares in the vault
/// * `user_id` - The address of the user
/// * `user_shares` - The number of shares the user has in the vault
pub fn update_rewards(e: &Env, total_shares: i128, user_id: &Address, user_shares: i128) {
    if let Some(reward_token) = storage::get_reward_token(e) {
        if let Some(reward_data) = update_reward_data(e, &reward_token, total_shares) {
            update_user_rewards(e, &reward_token, &reward_data, user_id, user_shares, false);
        }
    }
}

/// Claims rewards for a user
///
/// ### Arguments
/// * `total_shares` - The total number of shares in the vault
/// * `user_id` - The address of the user
/// * `user_shares` - The number of shares the user has in the vault
/// * `to` - The address to send the rewards to
///
/// Returns the number of tokens that need to be transferred to `user`
///
/// Panics if the vault never had rewards configured
pub fn claim_rewards(
    e: &Env,
    total_shares: i128,
    user_id: &Address,
    user_shares: i128,
    to: &Address,
) -> i128 {
    if let Some(reward_token) = storage::get_reward_token(e) {
        if let Some(reward_data) = update_reward_data(e, &reward_token, total_shares) {
            let reward_amount =
                update_user_rewards(e, &reward_token, &reward_data, user_id, user_shares, true);
            if reward_amount > 0 {
                TokenClient::new(e, &reward_token).transfer(
                    &e.current_contract_address(),
                    to,
                    &reward_amount,
                );
            }
            return reward_amount;
        }
    }
    panic_with_error!(e, FeeVaultError::NoRewardsConfigured)
}

/// Set the rewards for the vault.
///
/// ### Arguments
/// * `e` - The environment
/// * `from` - The address from which the rewards are being sent
/// * `total_shares` - The total number of shares in the vault
/// * `reward_token` - The address of the reward token
/// * `reward_amount` - The amount of rewards to set
/// * `expiration` - The expiration timestamp for the rewards
pub fn set_rewards(
    e: &Env,
    from: &Address,
    total_shares: i128,
    reward_token: &Address,
    reward_amount: i128,
    expiration: u64,
) {
    let reward_period = expiration - e.ledger().timestamp();
    if reward_period <= 0 || reward_amount <= 0 {
        panic_with_error!(e, FeeVaultError::InvalidRewardConfig);
    }

    // Transfer token amount into the vault
    TokenClient::new(e, reward_token).transfer(
        &from,
        &e.current_contract_address(),
        &reward_amount,
    );

    // Check if any rewards are active. If rewards are active, the reward token must match the current one,
    // and the expiration must be greater than the current timestamp for the new rewards to be applied.
    if let Some(cur_reward_token) = storage::get_reward_token(e) {
        if let Some(cur_reward_data) = update_reward_data(e, &cur_reward_token, total_shares) {
            if cur_reward_data.expiration > e.ledger().timestamp() {
                // active rewards!

                // validate the new settings can be used to update the current rewards
                if cur_reward_token != *reward_token || expiration < cur_reward_data.expiration {
                    panic_with_error!(e, FeeVaultError::InvalidRewardConfig);
                }

                // update the current rewards
                let cur_reward_time_left = cur_reward_data.expiration - e.ledger().timestamp();
                let to_emit_total =
                    reward_amount + cur_reward_time_left as i128 * cur_reward_data.eps as i128;
                let new_eps = calculate_eps(e, to_emit_total, reward_period);
                let new_reward_data = RewardData {
                    eps: new_eps,
                    expiration,
                    index: cur_reward_data.index,
                    last_time: e.ledger().timestamp(),
                };
                storage::set_reward_data(e, reward_token, &new_reward_data);
                return; // return to prevent fallthrough to the next section
            }
        }
    }

    // No active rewards found!
    // Set new reward data based on config. We need to check if any
    // old reward data exists, to persist the last calculated index.

    // An extra read occurs here of `reward_data` for expired rewards, but is left to keep the implementation
    // as simple as possible.
    let reward_index =
        if let Some(cur_reward_data) = update_reward_data(e, &reward_token, total_shares) {
            cur_reward_data.index
        } else {
            0
        };
    let new_eps = calculate_eps(e, reward_amount, reward_period);
    // Set the new reward data and reward token
    let new_reward_data = RewardData {
        eps: new_eps as u64,
        expiration,
        index: reward_index,
        last_time: e.ledger().timestamp(),
    };
    storage::set_reward_data(e, reward_token, &new_reward_data);
    storage::set_reward_token(e, reward_token);
}

/// Load an updated reward data for the given reward token.
///
/// This does NOT write the updated reward data to storage.
///
/// ### Arguments
/// * `reward_token` - The address of the reward token
/// * `total_shares` - The total number of shares in the vault
///
/// ### Returns
/// * `Option<RewardData>` - The updated reward data if it exists
pub fn load_updated_reward_data(
    e: &Env,
    reward_token: &Address,
    total_shares: i128,
) -> Option<RewardData> {
    match storage::get_reward_data(e, reward_token) {
        Some(reward_data) => {
            if reward_data.last_time >= reward_data.expiration
                || e.ledger().timestamp() == reward_data.last_time
                || reward_data.eps == 0
                || total_shares == 0
            {
                //  already updated or expired
                return Some(reward_data);
            }

            let max_timestamp = if e.ledger().timestamp() > reward_data.expiration {
                reward_data.expiration
            } else {
                e.ledger().timestamp()
            };

            let additional_idx = ((max_timestamp - reward_data.last_time) as i128
                * reward_data.eps as i128)
                .fixed_div_floor(total_shares, SCALAR_7)
                .unwrap_optimized();
            let new_data = RewardData {
                eps: reward_data.eps,
                expiration: reward_data.expiration,
                index: additional_idx + reward_data.index,
                last_time: e.ledger().timestamp(),
            };
            Some(new_data)
        }
        None => return None, // no reward exist, no update is required
    }
}

/***** Helper Functions *****/

/// Update the vault rewards index for vault shares
fn update_reward_data(e: &Env, reward_token: &Address, total_shares: i128) -> Option<RewardData> {
    match load_updated_reward_data(e, reward_token, total_shares) {
        Some(reward_data) => {
            storage::set_reward_data(e, &reward_token, &reward_data);
            Some(reward_data)
        }
        None => return None, // no reward exist, no update is required
    }
}

/// Update the user's rewards. If `to_claim` is true, the user's accrued rewards will be returned and
/// a value of zero will be stored to the ledger.
///
/// ### Arguments d
/// * `token` - The address of the reward token
/// * `reward_data` - The current reward data for the token
/// * `user` - The address of the user
/// * `user_shares` - The number of shares the user has in the pool
/// * `to_claim` - Whether the user is claiming their rewards
///
/// ### Returns
/// The number of claimed tokens the caller needs to send to the user
fn update_user_rewards(
    e: &Env,
    token: &Address,
    reward_data: &RewardData,
    user: &Address,
    user_shares: i128,
    to_claim: bool,
) -> i128 {
    if let Some(user_data) = storage::get_user_rewards(e, token, user) {
        if user_data.index != reward_data.index || to_claim {
            let mut accrual = user_data.accrued;
            if user_shares != 0 && reward_data.index > user_data.index {
                let delta_index = reward_data.index - user_data.index;
                let to_accrue = user_shares
                    .fixed_mul_floor(delta_index, SCALAR_7)
                    .unwrap_optimized();
                accrual += to_accrue;
            }
            return set_user_rewards(e, token, user, reward_data.index, accrual, to_claim);
        }
        // no accrual occurred and no claim requested
        return 0;
    } else if user_shares == 0 {
        // first time the user registered an action since rewards were added
        return set_user_rewards(e, token, user, reward_data.index, 0, to_claim);
    } else {
        // user had shares before rewards began, they are due any historical rewards
        let to_accrue = user_shares
            .fixed_mul_floor(reward_data.index, SCALAR_7)
            .unwrap_optimized();
        return set_user_rewards(e, token, user, reward_data.index, to_accrue, to_claim);
    }
}

fn set_user_rewards(
    e: &Env,
    token: &Address,
    user: &Address,
    index: i128,
    accrued: i128,
    to_claim: bool,
) -> i128 {
    if to_claim {
        storage::set_user_rewards(e, token, user, &UserRewards { index, accrued: 0 });
        accrued
    } else {
        storage::set_user_rewards(e, token, user, &UserRewards { index, accrued });
        0
    }
}

fn calculate_eps(e: &Env, reward_amount: i128, reward_period: u64) -> u64 {
    let new_eps = reward_amount / reward_period as i128;
    if new_eps <= 0 || new_eps > u64::MAX as i128 {
        panic_with_error!(e, FeeVaultError::InvalidRewardConfig);
    }
    new_eps as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::create_test_fee_vault;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        token::StellarAssetClient,
    };

    /********** update_rewards **********/

    #[test]
    fn test_update_rewards() {
        let e = Env::default();
        e.mock_all_auths();
        let block_timestamp = 1713139200 + 1234;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let reward_token = Address::generate(&e);
        let (vault_address, _, _) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        let samwise = Address::generate(&e);

        let last_timestamp = 1713139200;
        let reward_data = RewardData {
            expiration: last_timestamp + 7 * 24 * 60 * 60,
            eps: 0_1000000,
            index: 22222,
            last_time: last_timestamp,
        };
        let user_rewards_data = UserRewards {
            index: 11111,
            accrued: 3,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &reward_token);
            storage::set_reward_data(&e, &reward_token, &reward_data);
            storage::set_user_rewards(&e, &reward_token, &samwise, &user_rewards_data);

            let total_shares: i128 = 150_0000000;
            let user_balance: i128 = 9_0000000;
            update_rewards(&e, total_shares, &samwise, user_balance);

            let new_rewards_data = storage::get_reward_data(&e, &reward_token).unwrap_optimized();
            let new_user_data =
                storage::get_user_rewards(&e, &reward_token, &samwise).unwrap_optimized();
            assert_eq!(new_rewards_data.last_time, block_timestamp);
            assert_eq!(new_rewards_data.index, 8248888);
            assert_eq!(new_user_data.accrued, 7_4139996);
            assert_eq!(new_user_data.index, 8248888);
        });
    }

    #[test]
    fn test_update_rewards_no_data() {
        let e = Env::default();
        e.mock_all_auths();
        let block_timestamp = 1713139200 + 1234;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let reward_token = Address::generate(&e);
        let (vault_address, _, _) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        let samwise = Address::generate(&e);

        e.as_contract(&vault_address, || {
            let total_shares: i128 = 150_0000000;
            let user_balance: i128 = 9_0000000;
            update_rewards(&e, total_shares, &samwise, user_balance);

            let new_rewards_data = storage::get_reward_data(&e, &reward_token);
            let new_user_data = storage::get_user_rewards(&e, &reward_token, &samwise);
            assert!(new_rewards_data.is_none());
            assert!(new_user_data.is_none());
        });
    }

    #[test]
    fn test_update_rewards_expired_data() {
        let e = Env::default();
        e.mock_all_auths();
        let block_timestamp = 1713139200 + 1234;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let reward_token = Address::generate(&e);
        let (vault_address, _, _) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        let samwise = Address::generate(&e);

        let last_timestamp = 1713139200;
        let reward_data = RewardData {
            expiration: last_timestamp,
            eps: 0_1000000,
            index: 22222,
            last_time: last_timestamp,
        };
        let user_rewards_data = UserRewards {
            index: 11111,
            accrued: 3,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &reward_token);
            storage::set_reward_data(&e, &reward_token, &reward_data);
            storage::set_user_rewards(&e, &reward_token, &samwise, &user_rewards_data);

            let total_shares: i128 = 150_0000000;
            let user_balance: i128 = 9_0000000;
            update_rewards(&e, total_shares, &samwise, user_balance);

            let new_rewards_data = storage::get_reward_data(&e, &reward_token).unwrap_optimized();
            let new_user_data =
                storage::get_user_rewards(&e, &reward_token, &samwise).unwrap_optimized();
            assert_eq!(new_rewards_data.last_time, reward_data.last_time);
            assert_eq!(new_rewards_data.index, reward_data.index);
            assert_eq!(new_user_data.accrued, 99999 + 3);
            assert_eq!(new_user_data.index, reward_data.index);
        });
    }

    #[test]
    fn test_update_rewards_first_action() {
        let e = Env::default();
        e.mock_all_auths();
        let block_timestamp = 1713139200 + 12345;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let reward_token = Address::generate(&e);
        let (vault_address, _, _) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        let samwise = Address::generate(&e);

        let last_timestamp = 1713139200;
        let reward_data = RewardData {
            expiration: last_timestamp + 7 * 24 * 60 * 60,
            eps: 0_0420000,
            index: 22222,
            last_time: last_timestamp,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &reward_token);
            storage::set_reward_data(&e, &reward_token, &reward_data);

            let total_shares: i128 = 150_0000000;
            let user_balance: i128 = 0;
            update_rewards(&e, total_shares, &samwise, user_balance);

            let new_rewards_data = storage::get_reward_data(&e, &reward_token).unwrap_optimized();
            let new_user_data =
                storage::get_user_rewards(&e, &reward_token, &samwise).unwrap_optimized();
            assert_eq!(new_rewards_data.last_time, block_timestamp);
            assert_eq!(new_rewards_data.index, 34588222);
            assert_eq!(new_user_data.accrued, 0);
            assert_eq!(new_user_data.index, 34588222);
        });
    }

    #[test]
    fn test_update_rewards_config_set_after_user() {
        let e = Env::default();
        e.mock_all_auths();
        let block_timestamp = 1713139200 + 12345;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let reward_token = Address::generate(&e);
        let (vault_address, _, _) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        let samwise = Address::generate(&e);

        let last_timestamp = 1713139200;
        let reward_data = RewardData {
            expiration: last_timestamp + 7 * 24 * 60 * 60,
            eps: 0_0420000,
            index: 0,
            last_time: last_timestamp,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &reward_token);
            storage::set_reward_data(&e, &reward_token, &reward_data);

            let total_shares: i128 = 150_0000000;
            let user_balance: i128 = 9_0000000;
            update_rewards(&e, total_shares, &samwise, user_balance);

            let new_rewards_data = storage::get_reward_data(&e, &reward_token).unwrap_optimized();
            let new_user_data =
                storage::get_user_rewards(&e, &reward_token, &samwise).unwrap_optimized();
            assert_eq!(new_rewards_data.last_time, block_timestamp);
            assert_eq!(new_rewards_data.index, 34566000);
            assert_eq!(new_user_data.accrued, 31_1094000);
            assert_eq!(new_user_data.index, 34566000);
        });
    }

    #[test]
    fn test_update_rewards_zero_shares() {
        let e = Env::default();
        e.mock_all_auths();
        let block_timestamp = 1713139200 + 1234;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let reward_token = Address::generate(&e);
        let (vault_address, _, _) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        let samwise = Address::generate(&e);

        let last_timestamp = 1713139200;
        let reward_data = RewardData {
            expiration: last_timestamp + 7 * 24 * 60 * 60,
            eps: 0_1000000,
            index: 22222,
            last_time: last_timestamp,
        };
        let user_rewards_data = UserRewards {
            index: 11111,
            accrued: 3,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &reward_token);
            storage::set_reward_data(&e, &reward_token, &reward_data);
            storage::set_user_rewards(&e, &reward_token, &samwise, &user_rewards_data);

            let total_shares: i128 = 0;
            let user_balance: i128 = 0;
            update_rewards(&e, total_shares, &samwise, user_balance);

            let new_rewards_data = storage::get_reward_data(&e, &reward_token).unwrap_optimized();
            let new_user_data =
                storage::get_user_rewards(&e, &reward_token, &samwise).unwrap_optimized();
            assert_eq!(new_rewards_data.last_time, last_timestamp);
            assert_eq!(new_rewards_data.index, 22222);
            assert_eq!(new_user_data.accrued, 3);
            assert_eq!(new_user_data.index, 22222);
        });
    }

    /********** claim_rewards **********/

    #[test]
    fn test_claim_rewards() {
        let e = Env::default();
        e.mock_all_auths();
        let block_timestamp = 1713139200 + 1234;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let (vault_address, _, _) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        let starting_balance = 100_000_0000000;
        StellarAssetClient::new(&e, &reward_token).mint(&vault_address, &starting_balance);

        let samwise = Address::generate(&e);
        let frodo = Address::generate(&e);

        let last_timestamp = 1713139200;
        let reward_data = RewardData {
            expiration: last_timestamp + 7 * 24 * 60 * 60,
            eps: 0_1000000,
            index: 22222,
            last_time: last_timestamp,
        };
        let user_rewards_data = UserRewards {
            index: 11111,
            accrued: 3,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &reward_token);
            storage::set_reward_data(&e, &reward_token, &reward_data);
            storage::set_user_rewards(&e, &reward_token, &samwise, &user_rewards_data);

            let total_shares: i128 = 150_0000000;
            let user_balance: i128 = 9_0000000;
            storage::set_vault_shares(&e, &samwise, user_balance);

            let result = claim_rewards(&e, total_shares, &samwise, user_balance, &frodo);

            let new_rewards_data = storage::get_reward_data(&e, &reward_token).unwrap_optimized();
            let new_user_data =
                storage::get_user_rewards(&e, &reward_token, &samwise).unwrap_optimized();
            assert_eq!(new_rewards_data.last_time, block_timestamp);
            assert_eq!(new_rewards_data.index, 8248888);
            assert_eq!(result, 7_4139996);
            assert_eq!(new_user_data.accrued, 0);
            assert_eq!(new_user_data.index, 8248888);

            let token_client = TokenClient::new(&e, &reward_token);
            let frodo_balance = token_client.balance(&frodo);
            assert_eq!(frodo_balance, result);
            let contract_balance = token_client.balance(&vault_address);
            assert_eq!(contract_balance, starting_balance - result);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #110)")]
    fn test_claim_rewards_no_data() {
        let e = Env::default();
        e.mock_all_auths();
        let block_timestamp = 1713139200 + 1234;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let (vault_address, _, _) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        let starting_balance = 100_000_0000000;
        StellarAssetClient::new(&e, &reward_token).mint(&vault_address, &starting_balance);

        let samwise = Address::generate(&e);
        let frodo = Address::generate(&e);

        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &reward_token);

            let total_shares: i128 = 150_0000000;
            let user_balance: i128 = 9_0000000;
            claim_rewards(&e, total_shares, &samwise, user_balance, &frodo);
        });
    }

    #[test]
    fn test_claim_rewards_zero_accrued() {
        let e = Env::default();
        e.mock_all_auths();
        let block_timestamp = 1713139200 + 1234;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let (vault_address, _, _) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        let starting_balance = 100_000_0000000;
        StellarAssetClient::new(&e, &reward_token).mint(&vault_address, &starting_balance);

        let samwise = Address::generate(&e);
        let frodo = Address::generate(&e);

        let last_timestamp = 1713139200;
        let reward_data = RewardData {
            expiration: last_timestamp + 7 * 24 * 60 * 60,
            eps: 0_1000000,
            index: 22222,
            last_time: last_timestamp,
        };
        let user_rewards_data = UserRewards {
            index: 22222,
            accrued: 0,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &reward_token);
            storage::set_reward_data(&e, &reward_token, &reward_data);
            storage::set_user_rewards(&e, &reward_token, &samwise, &user_rewards_data);

            let total_shares: i128 = 150_0000000;
            let user_balance: i128 = 0;
            storage::set_vault_shares(&e, &samwise, user_balance);

            let result = claim_rewards(&e, total_shares, &samwise, user_balance, &frodo);

            let new_rewards_data = storage::get_reward_data(&e, &reward_token).unwrap_optimized();
            let new_user_data =
                storage::get_user_rewards(&e, &reward_token, &samwise).unwrap_optimized();
            assert_eq!(new_rewards_data.last_time, block_timestamp);
            assert_eq!(new_rewards_data.index, 8248888);
            assert_eq!(result, 0);
            assert_eq!(new_user_data.accrued, 0);
            assert_eq!(new_user_data.index, 8248888);

            let token_client = TokenClient::new(&e, &reward_token);
            let frodo_balance = token_client.balance(&frodo);
            assert_eq!(frodo_balance, result);
            let contract_balance = token_client.balance(&vault_address);
            assert_eq!(contract_balance, starting_balance);
        });
    }

    // @dev: The below tests should be impossible states to reach, but are left
    //       in to ensure any bad state does not result in incorrect rewards.

    #[test]
    #[should_panic(expected = "attempt to subtract with overflow")]
    fn test_update_rewards_negative_time_dif() {
        let e = Env::default();
        e.mock_all_auths();
        let block_timestamp = 1713139200 + 1234;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let reward_token = Address::generate(&e);
        let (vault_address, _, _) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        let samwise = Address::generate(&e);

        let last_timestamp = 1713139200;
        let reward_data = RewardData {
            expiration: last_timestamp + 7 * 24 * 60 * 60,
            eps: 0_1000000,
            index: 22222,
            last_time: block_timestamp + 1,
        };
        let user_rewards_data = UserRewards {
            index: 11111,
            accrued: 3,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &reward_token);
            storage::set_reward_data(&e, &reward_token, &reward_data);
            storage::set_user_rewards(&e, &reward_token, &samwise, &user_rewards_data);

            let total_shares: i128 = 150_0000000;
            let user_balance: i128 = 9_0000000;
            update_rewards(&e, total_shares, &samwise, user_balance);
        });
    }

    #[test]
    fn test_update_rewards_negative_user_index_corrects() {
        let e = Env::default();
        e.mock_all_auths();
        let block_timestamp = 1713139200 + 1234;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let reward_token = Address::generate(&e);
        let (vault_address, _, _) = create_test_fee_vault(&e, &bombadil, 0, 0_1000000, None);

        let samwise = Address::generate(&e);

        let last_timestamp = 1713139200;
        let reward_data = RewardData {
            expiration: last_timestamp + 7 * 24 * 60 * 60,
            eps: 0_1000000,
            index: 22222,
            last_time: last_timestamp,
        };
        let user_rewards_data = UserRewards {
            index: 8248888 + 1,
            accrued: 3,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &reward_token);
            storage::set_reward_data(&e, &reward_token, &reward_data);
            storage::set_user_rewards(&e, &reward_token, &samwise, &user_rewards_data);

            let total_shares: i128 = 150_0000000;
            let user_balance: i128 = 9_0000000;
            update_rewards(&e, total_shares, &samwise, user_balance);

            let new_rewards_data = storage::get_reward_data(&e, &reward_token).unwrap_optimized();
            let new_user_data =
                storage::get_user_rewards(&e, &reward_token, &samwise).unwrap_optimized();
            assert_eq!(new_rewards_data.last_time, block_timestamp);
            assert_eq!(new_rewards_data.index, 8248888);
            assert_eq!(new_user_data.accrued, 3);
            assert_eq!(new_user_data.index, 8248888);
        });
    }

    /********** set_rewards **********/

    #[test]
    fn test_set_rewards_first_time() {
        let e = Env::default();
        e.mock_all_auths_allowing_non_root_auth();
        let block_timestamp = 1713139200;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let (vault_address, _, _) = create_test_fee_vault(&e, &samwise, 0, 0_1000000, None);

        let reward_amount = 100_000_0000000;
        let expiration = block_timestamp + 30 * 24 * 60 * 60;
        StellarAssetClient::new(&e, &reward_token).mint(&samwise, &reward_amount);

        e.as_contract(&vault_address, || {
            let total_shares: i128 = 150_0000000;
            set_rewards(
                &e,
                &samwise,
                total_shares,
                &reward_token,
                reward_amount,
                expiration,
            );

            let new_reward_token = storage::get_reward_token(&e).unwrap_optimized();
            let new_rewards_data = storage::get_reward_data(&e, &reward_token).unwrap_optimized();
            assert_eq!(new_reward_token, reward_token);

            assert_eq!(new_rewards_data.last_time, block_timestamp);
            assert_eq!(new_rewards_data.index, 0);
            assert_eq!(new_rewards_data.eps, 0_0385802);
            assert_eq!(new_rewards_data.expiration, expiration);

            let reward_token_client = TokenClient::new(&e, &reward_token);
            assert_eq!(reward_amount, reward_token_client.balance(&vault_address));
            assert_eq!(reward_token_client.balance(&samwise), 0);
        });
    }

    #[test]
    fn test_set_rewards_same_token_active() {
        let e = Env::default();
        e.mock_all_auths_allowing_non_root_auth();
        let block_timestamp = 1713139200;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let (vault_address, _, _) = create_test_fee_vault(&e, &samwise, 0, 0_1000000, None);

        let reward_amount = 100_000_0000000;
        let expiration = block_timestamp + 30 * 24 * 60 * 60;
        StellarAssetClient::new(&e, &reward_token).mint(&samwise, &reward_amount);
        let last_timestamp = block_timestamp - 1234;
        let reward_data = RewardData {
            expiration: block_timestamp + 7 * 24 * 60 * 60,
            eps: 0_1000000,
            index: 22222,
            last_time: last_timestamp,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &reward_token);
            storage::set_reward_data(&e, &reward_token, &reward_data);

            let total_shares: i128 = 150_0000000;
            set_rewards(
                &e,
                &samwise,
                total_shares,
                &reward_token,
                reward_amount,
                expiration,
            );

            let new_reward_token = storage::get_reward_token(&e).unwrap_optimized();
            let new_rewards_data = storage::get_reward_data(&e, &reward_token).unwrap_optimized();
            assert_eq!(new_reward_token, reward_token);

            assert_eq!(new_rewards_data.last_time, block_timestamp);
            assert_eq!(new_rewards_data.index, 8248888); // index updated from prev data
            assert_eq!(new_rewards_data.eps, 0_0385802 + 0_0233333);
            assert_eq!(new_rewards_data.expiration, expiration);

            let reward_token_client = TokenClient::new(&e, &reward_token);
            assert_eq!(reward_amount, reward_token_client.balance(&vault_address));
            assert_eq!(reward_token_client.balance(&samwise), 0);
        });
    }

    #[test]
    fn test_set_rewards_same_token_expired() {
        let e = Env::default();
        e.mock_all_auths_allowing_non_root_auth();
        let block_timestamp = 1713139200;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let (vault_address, _, _) = create_test_fee_vault(&e, &samwise, 0, 0_1000000, None);

        let reward_amount = 100_000_0000000;
        let expiration = block_timestamp + 30 * 24 * 60 * 60;
        StellarAssetClient::new(&e, &reward_token).mint(&samwise, &reward_amount);
        let last_timestamp = block_timestamp - 7 * 24 * 60 * 60;
        let reward_data = RewardData {
            expiration: last_timestamp,
            eps: 0_1000000,
            index: 123456789,
            last_time: last_timestamp,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &reward_token);
            storage::set_reward_data(&e, &reward_token, &reward_data);

            let total_shares: i128 = 150_0000000;
            set_rewards(
                &e,
                &samwise,
                total_shares,
                &reward_token,
                reward_amount,
                expiration,
            );

            let new_reward_token = storage::get_reward_token(&e).unwrap_optimized();
            let new_rewards_data = storage::get_reward_data(&e, &reward_token).unwrap_optimized();
            assert_eq!(new_reward_token, reward_token);

            assert_eq!(new_rewards_data.last_time, block_timestamp);
            assert_eq!(new_rewards_data.index, 123456789); // index maintained from prev data
            assert_eq!(new_rewards_data.eps, 0_0385802);
            assert_eq!(new_rewards_data.expiration, expiration);

            let reward_token_client = TokenClient::new(&e, &reward_token);
            assert_eq!(reward_amount, reward_token_client.balance(&vault_address));
            assert_eq!(reward_token_client.balance(&samwise), 0);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #111)")]
    fn test_set_rewards_new_token_active() {
        let e = Env::default();
        e.mock_all_auths_allowing_non_root_auth();
        let block_timestamp = 1713139200;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let old_reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let (vault_address, _, _) = create_test_fee_vault(&e, &samwise, 0, 0_1000000, None);

        let reward_amount = 100_000_0000000;
        let expiration = block_timestamp + 30 * 24 * 60 * 60;
        StellarAssetClient::new(&e, &reward_token).mint(&samwise, &reward_amount);
        let last_timestamp = block_timestamp - 1234;
        let reward_data = RewardData {
            expiration: block_timestamp + 7 * 24 * 60 * 60,
            eps: 0_1000000,
            index: 22222,
            last_time: last_timestamp,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &old_reward_token);
            storage::set_reward_data(&e, &old_reward_token, &reward_data);

            let total_shares: i128 = 150_0000000;
            set_rewards(
                &e,
                &samwise,
                total_shares,
                &reward_token,
                reward_amount,
                expiration,
            );
        });
    }

    #[test]
    fn test_set_rewards_new_token_expired_first_time() {
        let e = Env::default();
        e.mock_all_auths_allowing_non_root_auth();
        let block_timestamp = 1713139200;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let old_reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let (vault_address, _, _) = create_test_fee_vault(&e, &samwise, 0, 0_1000000, None);

        let reward_amount = 100_000_0000000;
        let expiration = block_timestamp + 180 * 24 * 60 * 60;
        StellarAssetClient::new(&e, &reward_token).mint(&samwise, &reward_amount);
        let last_timestamp = block_timestamp - 7 * 24 * 60 * 60;
        let reward_data = RewardData {
            expiration: last_timestamp,
            eps: 0_1000000,
            index: 123456789,
            last_time: last_timestamp,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &old_reward_token);
            storage::set_reward_data(&e, &old_reward_token, &reward_data);

            let total_shares: i128 = 150_0000000;
            set_rewards(
                &e,
                &samwise,
                total_shares,
                &reward_token,
                reward_amount,
                expiration,
            );

            let new_reward_token = storage::get_reward_token(&e).unwrap_optimized();
            let new_rewards_data = storage::get_reward_data(&e, &reward_token).unwrap_optimized();
            assert_eq!(new_reward_token, reward_token);

            assert_eq!(new_rewards_data.last_time, block_timestamp);
            assert_eq!(new_rewards_data.index, 0);
            assert_eq!(new_rewards_data.eps, 0_0064300);
            assert_eq!(new_rewards_data.expiration, expiration);

            let reward_token_client = TokenClient::new(&e, &reward_token);
            assert_eq!(reward_amount, reward_token_client.balance(&vault_address));
            assert_eq!(reward_token_client.balance(&samwise), 0);
        });
    }

    #[test]
    fn test_set_rewards_new_token_expired() {
        let e = Env::default();
        e.mock_all_auths_allowing_non_root_auth();
        let block_timestamp = 1713139200;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let old_reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let (vault_address, _, _) = create_test_fee_vault(&e, &samwise, 0, 0_1000000, None);

        let reward_amount = 100_000_0000000;
        let expiration = block_timestamp + 180 * 24 * 60 * 60;
        StellarAssetClient::new(&e, &reward_token).mint(&samwise, &reward_amount);
        let last_timestamp = block_timestamp - 7 * 24 * 60 * 60;
        let reward_data = RewardData {
            expiration: last_timestamp,
            eps: 0_1000000,
            index: 123456789,
            last_time: last_timestamp,
        };
        let reward_data_old = RewardData {
            expiration: block_timestamp - 180 * 24 * 60 * 60,
            eps: 0_0050000,
            index: 987654321,
            last_time: block_timestamp - 180 * 24 * 60 * 60,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &old_reward_token);
            storage::set_reward_data(&e, &old_reward_token, &reward_data);
            storage::set_reward_data(&e, &reward_token, &reward_data_old);

            let total_shares: i128 = 150_0000000;
            set_rewards(
                &e,
                &samwise,
                total_shares,
                &reward_token,
                reward_amount,
                expiration,
            );

            let new_reward_token = storage::get_reward_token(&e).unwrap_optimized();
            let new_rewards_data = storage::get_reward_data(&e, &reward_token).unwrap_optimized();
            assert_eq!(new_reward_token, reward_token);

            assert_eq!(new_rewards_data.last_time, block_timestamp);
            assert_eq!(new_rewards_data.index, 987654321);
            assert_eq!(new_rewards_data.eps, 0_0064300);
            assert_eq!(new_rewards_data.expiration, expiration);

            let reward_token_client = TokenClient::new(&e, &reward_token);
            assert_eq!(reward_amount, reward_token_client.balance(&vault_address));
            assert_eq!(reward_token_client.balance(&samwise), 0);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #111)")]
    fn test_set_rewards_zero_eps() {
        let e = Env::default();
        e.mock_all_auths_allowing_non_root_auth();
        let block_timestamp = 1713139200;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let (vault_address, _, _) = create_test_fee_vault(&e, &samwise, 0, 0_1000000, None);

        let reward_amount = 1_0000000;
        let expiration = block_timestamp + 1_0000000 + 1;
        StellarAssetClient::new(&e, &reward_token).mint(&samwise, &reward_amount);

        e.as_contract(&vault_address, || {
            let total_shares: i128 = 150_0000000;
            set_rewards(
                &e,
                &samwise,
                total_shares,
                &reward_token,
                reward_amount,
                expiration,
            );
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #111)")]
    fn test_set_rewards_max_eps() {
        let e = Env::default();
        e.mock_all_auths_allowing_non_root_auth();
        let block_timestamp = 1713139200;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let (vault_address, _, _) = create_test_fee_vault(&e, &samwise, 0, 0_1000000, None);

        let reward_amount = i128::MAX / 2;
        let expiration = block_timestamp + 10;
        StellarAssetClient::new(&e, &reward_token).mint(&samwise, &reward_amount);

        e.as_contract(&vault_address, || {
            let total_shares: i128 = 150_0000000;
            set_rewards(
                &e,
                &samwise,
                total_shares,
                &reward_token,
                reward_amount,
                expiration,
            );
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #111)")]
    fn test_set_rewards_zero_reward_amount() {
        let e = Env::default();
        e.mock_all_auths_allowing_non_root_auth();
        let block_timestamp = 1713139200;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let (vault_address, _, _) = create_test_fee_vault(&e, &samwise, 0, 0_1000000, None);

        let reward_amount = 0;
        let expiration = block_timestamp + 7 * 24 * 60 * 60;
        StellarAssetClient::new(&e, &reward_token).mint(&samwise, &reward_amount);

        e.as_contract(&vault_address, || {
            let total_shares: i128 = 150_0000000;
            set_rewards(
                &e,
                &samwise,
                total_shares,
                &reward_token,
                reward_amount,
                expiration,
            );
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #111)")]
    fn test_set_rewards_active_checks_eps() {
        let e = Env::default();
        e.mock_all_auths_allowing_non_root_auth();
        let block_timestamp = 1713139200;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let (vault_address, _, _) = create_test_fee_vault(&e, &samwise, 0, 0_1000000, None);

        let reward_amount = 100;
        let expiration = block_timestamp + 30 * 24 * 60 * 60;
        StellarAssetClient::new(&e, &reward_token).mint(&samwise, &reward_amount);
        let last_timestamp = block_timestamp;
        let reward_data = RewardData {
            expiration: block_timestamp + 10000,
            eps: 0_0000100,
            index: 22222,
            last_time: last_timestamp,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &reward_token);
            storage::set_reward_data(&e, &reward_token, &reward_data);

            let total_shares: i128 = 150_0000000;
            set_rewards(
                &e,
                &samwise,
                total_shares,
                &reward_token,
                reward_amount,
                expiration,
            );
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #111)")]
    fn test_set_rewards_active_checks_expiration() {
        let e = Env::default();
        e.mock_all_auths_allowing_non_root_auth();
        let block_timestamp = 1713139200;
        e.ledger().set(LedgerInfo {
            timestamp: block_timestamp,
            protocol_version: 23,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10,
            min_persistent_entry_ttl: 10,
            max_entry_ttl: 3110400,
        });

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let reward_token = e
            .register_stellar_asset_contract_v2(bombadil.clone())
            .address();
        let (vault_address, _, _) = create_test_fee_vault(&e, &samwise, 0, 0_1000000, None);

        let reward_amount = 100_0000000;
        let expiration = block_timestamp + 30 * 24 * 60 * 60;
        StellarAssetClient::new(&e, &reward_token).mint(&samwise, &reward_amount);
        let last_timestamp = block_timestamp - 10000;
        let reward_data = RewardData {
            expiration: block_timestamp + 30 * 24 * 60 * 60 + 1,
            eps: 0_1000000,
            index: 22222,
            last_time: last_timestamp,
        };
        e.as_contract(&vault_address, || {
            storage::set_reward_token(&e, &reward_token);
            storage::set_reward_data(&e, &reward_token, &reward_data);

            let total_shares: i128 = 150_0000000;
            set_rewards(
                &e,
                &samwise,
                total_shares,
                &reward_token,
                reward_amount,
                expiration,
            );
        });
    }
}
