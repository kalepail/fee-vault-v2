use soroban_sdk::{contracttype, panic_with_error, unwrap::UnwrapOptimized, Address, Env, Symbol};

use crate::{errors::FeeVaultError, vault::VaultData};

//********** Storage Keys **********//

const POOL_KEY: &str = "Pool";
const ADMIN_KEY: &str = "Admin";
const ASSET_KEY: &str = "Asset";
const FEE_KEY: &str = "Fee";
const SIGNER_KEY: &str = "Signer";

const VAULT_DATA_KEY: &str = "Vault";

#[derive(Clone)]
#[contracttype]
pub enum FeeVaultDataKey {
    Shares(Address),
}

#[derive(Clone)]
#[contracttype]
pub struct Fee {
    /// The vault's fee mode
    /// * 0 = take rate (admin earns a percentage of the vault's earnings)
    /// * 1 = capped rate (vault earns at most the APR cap, with any additional returns going to the admin)
    /// * 2 = fixed rate (vault always earns the fixed rate, with the admin either supplmenting or earning the difference)
    pub rate_type: u32,
    /// The vault's fee rate, with 7 decimals (e.g. 1000000 = 10%)
    pub rate: u32,
}

//********** Storage Utils **********//

pub const ONE_DAY_LEDGERS: u32 = 17280; // assumes 5 seconds per ledger on average

const LEDGER_BUMP_SHARED: u32 = 31 * ONE_DAY_LEDGERS;
const LEDGER_THRESHOLD_SHARED: u32 = LEDGER_BUMP_SHARED - ONE_DAY_LEDGERS;

const LEDGER_BUMP_USER: u32 = 120 * ONE_DAY_LEDGERS;
const LEDGER_THRESHOLD_USER: u32 = LEDGER_BUMP_USER - 20 * ONE_DAY_LEDGERS;

/// Bump the instance lifetime by the defined amount
pub fn extend_instance(e: &Env) {
    e.storage()
        .instance()
        .extend_ttl(LEDGER_THRESHOLD_SHARED, LEDGER_BUMP_SHARED);
}

/********** Instance **********/

/// Get the pool address
pub fn get_pool(e: &Env) -> Address {
    e.storage()
        .instance()
        .get::<Symbol, Address>(&Symbol::new(e, POOL_KEY))
        .unwrap_optimized()
}

/// Set the pool address
pub fn set_pool(e: &Env, pool: Address) {
    e.storage()
        .instance()
        .set::<Symbol, Address>(&Symbol::new(e, POOL_KEY), &pool);
}

/// Get the admin address
pub fn get_admin(e: &Env) -> Address {
    e.storage()
        .instance()
        .get::<Symbol, Address>(&Symbol::new(e, ADMIN_KEY))
        .unwrap_optimized()
}

/// Set the admin address
pub fn set_admin(e: &Env, admin: Address) {
    e.storage()
        .instance()
        .set::<Symbol, Address>(&Symbol::new(e, ADMIN_KEY), &admin);
}

/// Get the asset address
pub fn get_asset(e: &Env) -> Address {
    e.storage()
        .instance()
        .get::<Symbol, Address>(&Symbol::new(e, ASSET_KEY))
        .unwrap_optimized()
}

/// Set the asset address
pub fn set_asset(e: &Env, asset: Address) {
    e.storage()
        .instance()
        .set::<Symbol, Address>(&Symbol::new(e, ASSET_KEY), &asset);
}

/// Get the fee mode for the fee vault
pub fn get_fee(e: &Env) -> Fee {
    e.storage()
        .instance()
        .get::<Symbol, Fee>(&Symbol::new(e, FEE_KEY))
        .unwrap_optimized()
}

/// Set the fee mode for the fee vault
pub fn set_fee(e: &Env, fee: Fee) {
    e.storage()
        .instance()
        .set::<Symbol, Fee>(&Symbol::new(e, FEE_KEY), &fee);
}

/// Get the signer address. Can be None if no signer is set.
pub fn get_signer(e: &Env) -> Option<Address> {
    e.storage()
        .instance()
        .get::<Symbol, Address>(&Symbol::new(e, SIGNER_KEY))
}

/// Set the signer address. If set, cannot be returned to None.
pub fn set_signer(e: &Env, signer: Address) {
    e.storage()
        .instance()
        .set::<Symbol, Address>(&Symbol::new(e, SIGNER_KEY), &signer);
}

/********** Persistent **********/

/// Set the vault data
///
/// ### Arguments
/// * `vault` - The vault data
pub fn set_vault_data(e: &Env, vault: &VaultData) {
    let key = Symbol::new(e, VAULT_DATA_KEY);
    e.storage()
        .persistent()
        .set::<Symbol, VaultData>(&key, vault);
    e.storage()
        .persistent()
        .extend_ttl(&key, LEDGER_THRESHOLD_USER, LEDGER_BUMP_USER);
}

/// Get the vault data
pub fn get_vault_data(e: &Env) -> VaultData {
    let key = Symbol::new(e, VAULT_DATA_KEY);
    let result = e.storage().persistent().get::<Symbol, VaultData>(&key);
    match result {
        Some(reserve_data) => {
            e.storage()
                .persistent()
                .extend_ttl(&key, LEDGER_THRESHOLD_USER, LEDGER_BUMP_USER);
            reserve_data
        }
        None => panic_with_error!(e, FeeVaultError::ReserveNotFound),
    }
}

/// Set the number of vault shares a user owns. Shares are stored with 7 decimal places of precision.
///
/// ### Arguments
/// * `user` - The address of the user
/// * `shares` - The number of shares the user owns
pub fn set_vault_shares(e: &Env, user: &Address, shares: i128) {
    let key = FeeVaultDataKey::Shares(user.clone());
    e.storage()
        .persistent()
        .set::<FeeVaultDataKey, i128>(&key, &shares);
    e.storage()
        .persistent()
        .extend_ttl(&key, LEDGER_THRESHOLD_USER, LEDGER_BUMP_USER);
}

/// Get the number of vault shares a user owns. Shares are stored with 7 decimal places of precision.
///
/// ### Arguments
/// * `user` - The address of the user
pub fn get_vault_shares(e: &Env, user: &Address) -> i128 {
    let key = FeeVaultDataKey::Shares(user.clone());
    let result = e.storage().persistent().get::<FeeVaultDataKey, i128>(&key);
    match result {
        Some(shares) => {
            e.storage()
                .persistent()
                .extend_ttl(&key, LEDGER_THRESHOLD_USER, LEDGER_BUMP_USER);
            shares
        }
        None => 0,
    }
}
