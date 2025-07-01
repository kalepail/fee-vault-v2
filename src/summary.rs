use blend_contract_sdk::pool::Client as PoolClient;
use soroban_fixed_point_math::SorobanFixedPoint;
use soroban_sdk::{contracttype, Address, Env};

use crate::{
    constants::{SCALAR_12, SCALAR_7},
    rewards::load_updated_reward_data,
    storage::{self, Fee, RewardData},
    vault::{self, VaultData},
};

/**
 * @dev
 *
 * Summary of the vault state. This is intended for offchain services like a dApp to easily display information
 * about the vault. It is not intended to be used for onchain logic.
 */

#[derive(Clone)]
#[contracttype]
pub struct VaultSummary {
    // The pool address
    pub pool: Address,
    // The asset address
    pub asset: Address,
    // The admin address
    pub admin: Address,
    // The signer address, or None if no signer is used
    pub signer: Option<Address>,
    // The fee mode and rate for the vault
    pub fee: Fee,
    // The vault data containing the current state of the vault
    pub vault: VaultData,
    // The estimate APR earned by suppliers to the vault
    pub est_apr: i128,
    // The reward token address, if any
    pub reward_token: Option<Address>,
    // The reward data for the reward_token, if any.
    // If none, the data contains all zeros due to Soroban option limitations
    pub reward_data: RewardData,
}

impl VaultSummary {
    /// Create a new VaultSummary
    pub fn load(e: &Env) -> Self {
        let pool = storage::get_pool(e);
        let asset = storage::get_asset(e);
        let admin = storage::get_admin(e);
        let signer = storage::get_signer(e);
        let fee = storage::get_fee(e);
        let vault = vault::get_vault_updated(e, &pool, &asset);

        let reward_token = storage::get_reward_token(e);
        let reward_data = if let Some(unwrapped_r_token) = reward_token.clone() {
            load_updated_reward_data(e, &unwrapped_r_token, vault.total_shares)
        } else {
            None
        };

        let reserve = PoolClient::new(e, &pool).get_reserve(&asset);
        let pool_config = PoolClient::new(e, &pool).get_config();

        // calc estimated APR for reserve in the vault
        // code pulled from https://github.com/blend-capital/blend-contracts-v2/blob/main/pool/src/pool/interest.rs#L23
        let liabilities = reserve
            .data
            .d_supply
            .fixed_mul_ceil(e, &reserve.data.d_rate, &SCALAR_12);
        let supply = reserve
            .data
            .b_supply
            .fixed_mul_floor(e, &reserve.data.b_rate, &SCALAR_12);
        let cur_util: i128 = if liabilities == 0 {
            0
        } else if liabilities >= supply {
            SCALAR_7
        } else {
            liabilities.fixed_div_ceil(e, &supply, &SCALAR_7)
        };
        let cur_ir: i128;
        let target_util: i128 = reserve.config.util as i128;
        if cur_util <= target_util {
            let util_scalar = cur_util.fixed_div_ceil(e, &target_util, &SCALAR_7);
            let base_rate =
                util_scalar.fixed_mul_ceil(e, &(reserve.config.r_one as i128), &SCALAR_7)
                    + (reserve.config.r_base as i128);

            cur_ir = base_rate.fixed_mul_ceil(e, &reserve.data.ir_mod, &SCALAR_7);
        } else if cur_util <= 0_9500000 {
            let util_scalar =
                (cur_util - target_util).fixed_div_ceil(e, &(0_9500000 - target_util), &SCALAR_7);
            let base_rate =
                util_scalar.fixed_mul_ceil(e, &(reserve.config.r_two as i128), &SCALAR_7)
                    + (reserve.config.r_one as i128)
                    + (reserve.config.r_base as i128);

            cur_ir = base_rate.fixed_mul_ceil(e, &reserve.data.ir_mod, &SCALAR_7);
        } else {
            let util_scalar = (cur_util - 0_9500000).fixed_div_ceil(e, &0_0500000, &SCALAR_7);
            let extra_rate =
                util_scalar.fixed_mul_ceil(e, &(reserve.config.r_three as i128), &SCALAR_7);

            let intersection = reserve.data.ir_mod.fixed_mul_ceil(
                e,
                &((reserve.config.r_two + reserve.config.r_one + reserve.config.r_base) as i128),
                &SCALAR_7,
            );
            cur_ir = extra_rate + intersection;
        }

        // cur_ir is the borrow rate, convert to supply rate
        let supply_apr = cur_ir
            .fixed_mul_floor(e, &cur_util, &SCALAR_7)
            .fixed_mul_floor(e, &(SCALAR_7 - (pool_config.bstop_rate as i128)), &SCALAR_7);

        // check vault fee to get final est apr
        let est_apr = match fee.rate_type {
            0 => {
                // take rate
                supply_apr.fixed_mul_floor(e, &(SCALAR_7 - (fee.rate as i128)), &SCALAR_7)
            }
            1 => {
                // capped rate
                if supply_apr > (fee.rate as i128) {
                    fee.rate as i128 // capped rate
                } else {
                    supply_apr // no cap applied
                }
            }
            2 => {
                // fixed rate
                // rate applies if the admin has a balance or if the supply
                // apr exceeds the fixed rate
                if vault.admin_balance > 0 || (fee.rate as i128) < supply_apr {
                    fee.rate as i128
                } else {
                    // no admin balance and supply apr is less than the fixed rate
                    // behaves like a capped rate vault
                    supply_apr
                }
            }
            _ => 0,
        };

        VaultSummary {
            pool,
            asset,
            admin,
            signer,
            fee,
            vault,
            est_apr,
            reward_token,
            reward_data: reward_data.unwrap_or(RewardData {
                expiration: 0,
                eps: 0,
                last_time: 0,
                index: 0,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::{
        assert_approx_eq_abs,
        mockpool::{register_mock_pool_with_config_and_data, ReserveConfig, ReserveData},
        register_fee_vault, EnvTestUtils,
    };
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn test_vault_summary() {
        let e = Env::default();
        e.cost_estimate().budget().reset_unlimited();
        e.mock_all_auths();
        e.set_default_info();

        let bombadil = Address::generate(&e);
        let token = Address::generate(&e);

        let backstop_rate: u32 = 0_100_0000; // 10%
        let reserve_config = ReserveConfig {
            c_factor: 900_0000,
            decimals: 7,
            index: 0,
            l_factor: 900_0000,
            max_util: 900_0000,
            reactivity: 0,
            r_base: 30_0000,
            r_one: 60_0000,
            r_two: 120_0000,
            r_three: 5_000_0000,
            util: 0_800_0000,
            supply_cap: i64::MAX as i128,
            enabled: true,
        };
        // 85% util, 2.5x ir mod
        let reserve_data = ReserveData {
            b_supply: 100_0000000,
            b_rate: 1_500_000_000_000,
            d_supply: 63_7500000,
            d_rate: 2_000_000_000_000,
            ir_mod: 2_500_0000,
            backstop_credit: 0,
            last_time: e.ledger().timestamp(),
        };
        // expected borrow ir is ~32.5%
        let pool_client = register_mock_pool_with_config_and_data(
            &e,
            backstop_rate,
            reserve_config,
            reserve_data,
        );

        let rate_type = 0;
        let rate = 0_100_0000; // 10%
        let fee_vault = register_fee_vault(
            &e,
            &bombadil,
            &pool_client.address,
            &token,
            rate_type,
            rate,
            None,
        );

        e.as_contract(&fee_vault, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_500_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            let summary = VaultSummary::load(&e);
            assert_eq!(summary.pool, pool_client.address);
            assert_eq!(summary.asset, token);
            assert_eq!(summary.admin, bombadil);
            assert_eq!(summary.signer, None);
            assert_eq!(summary.fee.rate_type, rate_type);
            assert_eq!(summary.fee.rate, rate);
            assert_eq!(summary.vault.total_b_tokens, 1000_0000000);
            assert_eq!(summary.vault.total_shares, 1200_0000000);
            assert_eq!(summary.vault.b_rate, 1_500_000_000_000);
            assert_eq!(summary.vault.last_update_timestamp, e.ledger().timestamp());
            assert_eq!(summary.vault.admin_balance, 0);
            assert!(summary.reward_token.is_none());
            assert_eq!(summary.reward_data.eps, 0);
            assert_eq!(summary.reward_data.index, 0);
            assert_eq!(summary.reward_data.last_time, 0);
            assert_eq!(summary.reward_data.expiration, 0);
            // 0.325 * 0.85 * (1 - 0.1) * (1 - 0.1)
            assert_approx_eq_abs(summary.est_apr, 0_2237625, 0_0001000);
        });
    }

    #[test]
    fn test_vault_summary_fixed_rate_zero_take() {
        let e = Env::default();
        e.cost_estimate().budget().reset_unlimited();
        e.mock_all_auths();
        e.set_default_info();

        let bombadil = Address::generate(&e);
        let token = Address::generate(&e);

        let backstop_rate: u32 = 0;
        let reserve_config = ReserveConfig {
            c_factor: 900_0000,
            decimals: 7,
            index: 0,
            l_factor: 900_0000,
            max_util: 900_0000,
            reactivity: 0,
            r_base: 30_0000,
            r_one: 60_0000,
            r_two: 120_0000,
            r_three: 5_000_0000,
            util: 0_800_0000,
            supply_cap: i64::MAX as i128,
            enabled: true,
        };
        // 85% util, 2.5x ir mod
        let reserve_data = ReserveData {
            b_supply: 100_0000000,
            b_rate: 1_500_000_000_000,
            d_supply: 63_7500000,
            d_rate: 2_000_000_000_000,
            ir_mod: 2_500_0000,
            backstop_credit: 0,
            last_time: e.ledger().timestamp(),
        };
        // expected borrow ir is ~32.5%
        let pool_client = register_mock_pool_with_config_and_data(
            &e,
            backstop_rate,
            reserve_config,
            reserve_data,
        );

        let rate_type = 0;
        let rate = 0;
        let fee_vault = register_fee_vault(
            &e,
            &bombadil,
            &pool_client.address,
            &token,
            rate_type,
            rate,
            None,
        );

        e.as_contract(&fee_vault, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_500_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            let summary = VaultSummary::load(&e);
            // non apr summary result validated in test_vault_summary
            // 0.325 * 0.85
            assert_approx_eq_abs(summary.est_apr, 0_2762500, 0_0001000);
        });
    }

    #[test]
    fn test_vault_summary_capped_rate_below_cap() {
        let e = Env::default();
        e.cost_estimate().budget().reset_unlimited();
        e.mock_all_auths();
        e.set_default_info();

        let bombadil = Address::generate(&e);
        let token = Address::generate(&e);

        let backstop_rate: u32 = 0_200_0000;
        let reserve_config = ReserveConfig {
            c_factor: 900_0000,
            decimals: 7,
            index: 0,
            l_factor: 900_0000,
            max_util: 900_0000,
            reactivity: 0,
            r_base: 30_0000,
            r_one: 60_0000,
            r_two: 120_0000,
            r_three: 5_000_0000,
            util: 0_800_0000,
            supply_cap: i64::MAX as i128,
            enabled: true,
        };
        // 70% util, 0.75x ir mod
        let reserve_data = ReserveData {
            b_supply: 100_0000000,
            b_rate: 1_500_000_000_000,
            d_supply: 52_5000000,
            d_rate: 2_000_000_000_000,
            ir_mod: 0_750_0000,
            backstop_credit: 0,
            last_time: e.ledger().timestamp(),
        };
        // expected borrow ir is ~6.187%
        let pool_client = register_mock_pool_with_config_and_data(
            &e,
            backstop_rate,
            reserve_config,
            reserve_data,
        );

        let rate_type = 1;
        let rate = 0_040_0000;
        let fee_vault = register_fee_vault(
            &e,
            &bombadil,
            &pool_client.address,
            &token,
            rate_type,
            rate,
            None,
        );

        e.as_contract(&fee_vault, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_500_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            let summary = VaultSummary::load(&e);
            // non apr summary result validated in test_vault_summary
            // 0.06187 * 0.70 * 0.8
            assert_approx_eq_abs(summary.est_apr, 0_0346472, 0_0001000);
        });
    }

    #[test]
    fn test_vault_summary_capped_rate_above_cap() {
        let e = Env::default();
        e.cost_estimate().budget().reset_unlimited();
        e.mock_all_auths();
        e.set_default_info();

        let bombadil = Address::generate(&e);
        let token = Address::generate(&e);

        let backstop_rate: u32 = 0_200_0000;
        let reserve_config = ReserveConfig {
            c_factor: 900_0000,
            decimals: 7,
            index: 0,
            l_factor: 900_0000,
            max_util: 900_0000,
            reactivity: 0,
            r_base: 30_0000,
            r_one: 60_0000,
            r_two: 120_0000,
            r_three: 5_000_0000,
            util: 0_800_0000,
            supply_cap: i64::MAX as i128,
            enabled: true,
        };
        // 96% util, 0.75x ir mod
        let reserve_data = ReserveData {
            b_supply: 100_0000000,
            b_rate: 1_500_000_000_000,
            d_supply: 72_0000000,
            d_rate: 2_000_000_000_000,
            ir_mod: 0_750_0000,
            backstop_credit: 0,
            last_time: e.ledger().timestamp(),
        };
        // expected borrow ir is ~115.75%
        let pool_client = register_mock_pool_with_config_and_data(
            &e,
            backstop_rate,
            reserve_config,
            reserve_data,
        );

        let rate_type = 1;
        let rate = 0_150_0000;
        let fee_vault = register_fee_vault(
            &e,
            &bombadil,
            &pool_client.address,
            &token,
            rate_type,
            rate,
            None,
        );

        e.as_contract(&fee_vault, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_500_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            let summary = VaultSummary::load(&e);
            // non apr summary result validated in test_vault_summary
            // 115.75 * 0.96 * 0.8, capped at 15%
            assert_eq!(summary.est_apr, 0_150_0000);
        });
    }

    #[test]
    fn test_vault_summary_fixed_rate_below_cap_and_admin_balance() {
        let e = Env::default();
        e.cost_estimate().budget().reset_unlimited();
        e.mock_all_auths();
        e.set_default_info();

        let bombadil = Address::generate(&e);
        let token = Address::generate(&e);

        let backstop_rate: u32 = 0_200_0000;
        let reserve_config = ReserveConfig {
            c_factor: 900_0000,
            decimals: 7,
            index: 0,
            l_factor: 900_0000,
            max_util: 900_0000,
            reactivity: 0,
            r_base: 30_0000,
            r_one: 60_0000,
            r_two: 120_0000,
            r_three: 5_000_0000,
            util: 0_800_0000,
            supply_cap: i64::MAX as i128,
            enabled: true,
        };
        // 80% util, 1x ir mod
        let reserve_data = ReserveData {
            b_supply: 100_0000000,
            b_rate: 1_500_000_000_000,
            d_supply: 60_0000000,
            d_rate: 2_000_000_000_000,
            ir_mod: 1_000_0000,
            backstop_credit: 0,
            last_time: e.ledger().timestamp(),
        };
        // expected borrow ir is ~9%
        let pool_client = register_mock_pool_with_config_and_data(
            &e,
            backstop_rate,
            reserve_config,
            reserve_data,
        );

        let rate_type = 2;
        let rate = 0_070_0000;
        let fee_vault = register_fee_vault(
            &e,
            &bombadil,
            &pool_client.address,
            &token,
            rate_type,
            rate,
            None,
        );

        e.as_contract(&fee_vault, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_500_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 10000,
            };
            storage::set_vault_data(&e, &vault_data);

            let summary = VaultSummary::load(&e);
            // non apr summary result validated in test_vault_summary
            // 0.09 * 0.8, boosted to 0.07
            assert_eq!(summary.est_apr, 0_070_0000);
        });
    }

    #[test]
    fn test_vault_summary_fixed_rate_below_cap_and_no_admin_balance() {
        let e = Env::default();
        e.cost_estimate().budget().reset_unlimited();
        e.mock_all_auths();
        e.set_default_info();

        let bombadil = Address::generate(&e);
        let token = Address::generate(&e);

        let backstop_rate: u32 = 0_200_0000;
        let reserve_config = ReserveConfig {
            c_factor: 900_0000,
            decimals: 7,
            index: 0,
            l_factor: 900_0000,
            max_util: 900_0000,
            reactivity: 0,
            r_base: 30_0000,
            r_one: 60_0000,
            r_two: 120_0000,
            r_three: 5_000_0000,
            util: 0_800_0000,
            supply_cap: i64::MAX as i128,
            enabled: true,
        };
        // 80% util, 1x ir mod
        let reserve_data = ReserveData {
            b_supply: 100_0000000,
            b_rate: 1_500_000_000_000,
            d_supply: 60_0000000,
            d_rate: 2_000_000_000_000,
            ir_mod: 1_000_0000,
            backstop_credit: 0,
            last_time: e.ledger().timestamp(),
        };
        // expected borrow ir is ~9%
        let pool_client = register_mock_pool_with_config_and_data(
            &e,
            backstop_rate,
            reserve_config,
            reserve_data,
        );

        let rate_type = 2;
        let rate = 0_070_0000;
        let fee_vault = register_fee_vault(
            &e,
            &bombadil,
            &pool_client.address,
            &token,
            rate_type,
            rate,
            None,
        );

        e.as_contract(&fee_vault, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_500_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            };
            storage::set_vault_data(&e, &vault_data);

            let summary = VaultSummary::load(&e);
            // non apr summary result validated in test_vault_summary
            // 0.09 * 0.8 * 0.8, no boost applied
            assert_approx_eq_abs(summary.est_apr, 0_0576000, 0_0001000);
        });
    }
}
