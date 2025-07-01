#![cfg(test)]

use core::i64;

use crate::{constants::SCALAR_7, storage::ONE_DAY_LEDGERS, FeeVault};
use blend_contract_sdk::pool::{Client as PoolClient, ReserveConfig, ReserveEmissionMetadata};
use blend_contract_sdk::testutils::BlendFixture;
use sep_41_token::testutils::MockTokenClient;
use soroban_fixed_point_math::FixedPoint;
use soroban_sdk::{
    testutils::{Address as _, BytesN as _, Ledger as _, LedgerInfo},
    unwrap::UnwrapOptimized,
    vec, Address, BytesN, Env, String, Symbol,
};

/// Defaults to a mock pool with a b_rate of 1_100_000_000 and a take_rate of 0_1000000.
pub(crate) fn register_fee_vault(
    e: &Env,
    admin: &Address,
    pool: &Address,
    asset: &Address,
    rate_type: u32,
    rate: u32,
    signer: Option<Address>,
) -> Address {
    e.register(
        FeeVault {},
        (
            admin.clone(),
            pool.clone(),
            asset.clone(),
            rate_type,
            rate,
            signer,
        ),
    )
}

/// Create a test fee vault. If no initial b_rate is provided, it defaults to 1_100_000_000.
/// Uses a mock pool underneath so no deposits or withdrawls are functional.
///
/// Returns (vault address, mock pool address, mock token address)
pub(crate) fn create_test_fee_vault(
    e: &Env,
    admin: &Address,
    rate_type: u32,
    rate: u32,
    b_rate: Option<i128>,
) -> (Address, Address, Address) {
    let pool =
        mockpool::register_mock_pool_with_b_rate(e, b_rate.unwrap_or(1_100_000_000_000)).address;
    let asset = e
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let vault = register_fee_vault(e, &admin, &pool, &asset, rate_type, rate, None);
    (vault, pool, asset)
}

pub(crate) fn create_blend_pool(
    e: &Env,
    blend_fixture: &BlendFixture,
    admin: &Address,
    usdc: &MockTokenClient,
    xlm: &MockTokenClient,
) -> Address {
    // Mint usdc to admin
    usdc.mint(&admin, &200_000_0000000);
    // Mint xlm to admin
    xlm.mint(&admin, &200_000_0000000);

    // set up oracle
    let (oracle, oracle_client) = create_mock_oracle(e);
    oracle_client.set_data(
        &admin,
        &Asset::Other(Symbol::new(&e, "USD")),
        &vec![
            e,
            Asset::Stellar(usdc.address.clone()),
            Asset::Stellar(xlm.address.clone()),
        ],
        &7,
        &300,
    );
    oracle_client.set_price_stable(&vec![e, 1_000_0000, 100_0000]);
    let salt = BytesN::<32>::random(&e);
    let pool = blend_fixture.pool_factory.deploy(
        &admin,
        &String::from_str(e, "TEST"),
        &salt,
        &oracle,
        &0,
        &4,
        &1_0000000,
    );
    let pool_client = PoolClient::new(e, &pool);
    blend_fixture
        .backstop
        .deposit(&admin, &pool, &20_0000_0000000);
    let reserve_config = ReserveConfig {
        c_factor: 900_0000,
        decimals: 7,
        index: 0,
        l_factor: 900_0000,
        max_util: 900_0000,
        reactivity: 0,
        r_base: 100_0000,
        r_one: 0,
        r_two: 0,
        r_three: 0,
        util: 0,
        supply_cap: i64::MAX as i128,
        enabled: true,
    };
    pool_client.queue_set_reserve(&usdc.address, &reserve_config);
    pool_client.set_reserve(&usdc.address);
    pool_client.queue_set_reserve(&xlm.address, &reserve_config);
    pool_client.set_reserve(&xlm.address);
    let emission_config = vec![
        e,
        ReserveEmissionMetadata {
            res_index: 0,
            res_type: 0,
            share: 250_0000,
        },
        ReserveEmissionMetadata {
            res_index: 0,
            res_type: 1,
            share: 250_0000,
        },
        ReserveEmissionMetadata {
            res_index: 1,
            res_type: 0,
            share: 250_0000,
        },
        ReserveEmissionMetadata {
            res_index: 1,
            res_type: 1,
            share: 250_0000,
        },
    ];
    pool_client.set_emissions_config(&emission_config);
    pool_client.set_status(&0);
    blend_fixture.backstop.add_reward(&pool, &None);

    // wait a week and start emissions
    e.jump(ONE_DAY_LEDGERS * 7);
    blend_fixture.emitter.distribute();
    return pool;
}

pub trait EnvTestUtils {
    /// Jump the env by the given amount of ledgers. Assumes 5 seconds per ledger.
    fn jump(&self, ledgers: u32);

    /// Jump the env by the given amount of seconds. Incremends the sequence by 1.
    fn jump_time(&self, seconds: u64);

    /// Set the ledger to the default LedgerInfo
    ///
    /// Time -> 1441065600 (Sept 1st, 2015 12:00:00 AM UTC)
    /// Sequence -> 100
    fn set_default_info(&self);
}

impl EnvTestUtils for Env {
    fn jump(&self, ledgers: u32) {
        self.ledger().set(LedgerInfo {
            timestamp: self.ledger().timestamp().saturating_add(ledgers as u64 * 5),
            protocol_version: 22,
            sequence_number: self.ledger().sequence().saturating_add(ledgers),
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 30 * ONE_DAY_LEDGERS,
            min_persistent_entry_ttl: 30 * ONE_DAY_LEDGERS,
            max_entry_ttl: 365 * ONE_DAY_LEDGERS,
        });
    }

    fn jump_time(&self, seconds: u64) {
        self.ledger().set(LedgerInfo {
            timestamp: self.ledger().timestamp().saturating_add(seconds),
            protocol_version: 22,
            sequence_number: self.ledger().sequence().saturating_add(1),
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 30 * ONE_DAY_LEDGERS,
            min_persistent_entry_ttl: 30 * ONE_DAY_LEDGERS,
            max_entry_ttl: 365 * ONE_DAY_LEDGERS,
        });
    }

    fn set_default_info(&self) {
        self.ledger().set(LedgerInfo {
            timestamp: 1441065600, // Sept 1st, 2015 12:00:00 AM UTC
            protocol_version: 22,
            sequence_number: 100,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 30 * ONE_DAY_LEDGERS,
            min_persistent_entry_ttl: 30 * ONE_DAY_LEDGERS,
            max_entry_ttl: 365 * ONE_DAY_LEDGERS,
        });
    }
}

pub fn assert_approx_eq_abs(a: i128, b: i128, delta: i128) {
    assert!(
        a > b - delta && a < b + delta,
        "assertion failed: `(left != right)` \
         (left: `{:?}`, right: `{:?}`, epsilon: `{:?}`)",
        a,
        b,
        delta
    );
}

/// Asset that `b` is within `percentage` of `a` where `percentage`
/// is a percentage in decimal form as a fixed-point number with 7 decimal
/// places
pub fn assert_approx_eq_rel(a: i128, b: i128, percentage: i128) {
    let rel_delta = b.fixed_mul_floor(percentage, SCALAR_7).unwrap_optimized();

    assert!(
        a > b - rel_delta && a < b + rel_delta,
        "assertion failed: `(left != right)` \
         (left: `{:?}`, right: `{:?}`, epsilon: `{:?}`)",
        a,
        b,
        rel_delta
    );
}

/// Oracle
use sep_40_oracle::testutils::{Asset, MockPriceOracleClient, MockPriceOracleWASM};

pub fn create_mock_oracle<'a>(e: &Env) -> (Address, MockPriceOracleClient<'a>) {
    let contract_id = Address::generate(e);
    e.register_at(&contract_id, MockPriceOracleWASM, ());
    (
        contract_id.clone(),
        MockPriceOracleClient::new(e, &contract_id),
    )
}

/// Mock pool to test b_rate updates
pub mod mockpool {

    use soroban_sdk::{contract, contractimpl, contracttype, symbol_short, Address, Env, Symbol};

    use crate::constants::SCALAR_7;

    const BRATE: Symbol = symbol_short!("b_rate");
    const CONFIG: Symbol = symbol_short!("config");
    const DATA: Symbol = symbol_short!("data");
    const BACKSTOP_RATE: Symbol = symbol_short!("backstop");

    #[derive(Clone, Debug)]
    #[contracttype]
    pub struct Reserve {
        pub asset: Address,        // the underlying asset address
        pub config: ReserveConfig, // the reserve configuration
        pub data: ReserveData,     // the reserve data
        pub scalar: i128,
    }

    #[derive(Clone, Debug, Default)]
    #[contracttype]
    pub struct ReserveConfig {
        pub index: u32,       // the index of the reserve in the list
        pub decimals: u32,    // the decimals used in both the bToken and underlying contract
        pub c_factor: u32, // the collateral factor for the reserve scaled expressed in 7 decimals
        pub l_factor: u32, // the liability factor for the reserve scaled expressed in 7 decimals
        pub util: u32,     // the target utilization rate scaled expressed in 7 decimals
        pub max_util: u32, // the maximum allowed utilization rate scaled expressed in 7 decimals
        pub r_base: u32, // the R0 value (base rate) in the interest rate formula scaled expressed in 7 decimals
        pub r_one: u32,  // the R1 value in the interest rate formula scaled expressed in 7 decimals
        pub r_two: u32,  // the R2 value in the interest rate formula scaled expressed in 7 decimals
        pub r_three: u32, // the R3 value in the interest rate formula scaled expressed in 7 decimals
        pub reactivity: u32, // the reactivity constant for the reserve scaled expressed in 7 decimals
        pub supply_cap: i128, // the total amount of underlying tokens that can be used as collateral
        pub enabled: bool,    // the flag of the reserve
    }

    #[derive(Clone, Debug, Default)]
    #[contracttype]
    pub struct ReserveData {
        pub d_rate: i128,   // the conversion rate from dToken to underlying with 12 decimals
        pub b_rate: i128,   // the conversion rate from bToken to underlying with 12 decimals
        pub ir_mod: i128,   // the interest rate curve modifier with 7 decimals
        pub b_supply: i128, // the total supply of b tokens, in the underlying token's decimals
        pub d_supply: i128, // the total supply of d tokens, in the underlying token's decimals
        pub backstop_credit: i128, // the amount of underlying tokens currently owed to the backstop
        pub last_time: u64, // the last block the data was updated
    }

    #[derive(Clone, Debug)]
    #[contracttype]
    pub struct PoolConfig {
        pub oracle: Address,      // the contract address of the oracle
        pub min_collateral: i128, // the minimum amount of collateral required to open a liability position
        pub bstop_rate: u32, // the rate the backstop takes on accrued debt interest, expressed in 7 decimals
        pub status: u32,     // the status of the pool
        pub max_positions: u32, // the maximum number of effective positions a single user can hold, and the max assets an auction can contain
    }

    #[contract]
    pub struct MockPool;

    #[contractimpl]
    impl MockPool {
        /// Set the reserve b_rate. This overrides any set reserve data.
        pub fn set_b_rate(e: Env, b_rate: i128) {
            e.storage().instance().set(&BRATE, &b_rate);
        }

        /// Set the backstop rate
        pub fn set_backstop_rate(e: Env, bstop_rate: u32) {
            e.storage().instance().set(&BACKSTOP_RATE, &bstop_rate);
        }

        /// Set the reserve data. Clears any set b_rate
        pub fn set_data(e: Env, data: ReserveData) {
            if e.storage().instance().has(&BRATE) {
                e.storage().instance().remove(&BRATE);
            }
            e.storage().instance().set(&DATA, &data);
        }

        /// Set the reserve config
        pub fn set_config(e: Env, config: ReserveConfig) {
            e.storage().instance().set(&CONFIG, &config);
        }

        /// Note: All functionality only cares about the b_rate, except the vault summary.
        pub fn get_reserve(e: Env, reserve: Address) -> Reserve {
            let mut r_data = e
                .storage()
                .instance()
                .get(&DATA)
                .unwrap_or(ReserveData::default());
            if let Some(b_rate) = e.storage().instance().get(&BRATE) {
                r_data.b_rate = b_rate;
            }
            Reserve {
                asset: reserve,
                config: e
                    .storage()
                    .instance()
                    .get(&CONFIG)
                    .unwrap_or(ReserveConfig::default()),
                data: r_data,
                scalar: SCALAR_7,
            }
        }

        /// Note: We are only interested in the bstop_rate.
        pub fn get_config(e: Env) -> PoolConfig {
            PoolConfig {
                oracle: e.current_contract_address(),
                min_collateral: 0,
                bstop_rate: e.storage().instance().get(&BACKSTOP_RATE).unwrap_or(0),
                status: 0,
                max_positions: 4,
            }
        }
    }

    pub fn register_mock_pool_with_b_rate(e: &Env, b_rate: i128) -> MockPoolClient {
        let pool_address = e.register(MockPool {}, ());
        let client = MockPoolClient::new(e, &pool_address);
        client.set_b_rate(&b_rate);
        client
    }

    pub fn register_mock_pool_with_config_and_data(
        e: &Env,
        bstop_rate: u32,
        config: ReserveConfig,
        data: ReserveData,
    ) -> MockPoolClient {
        let pool_address = e.register(MockPool {}, ());
        let client = MockPoolClient::new(e, &pool_address);
        client.set_backstop_rate(&bstop_rate);
        client.set_config(&config);
        client.set_data(&data);
        client
    }
}
