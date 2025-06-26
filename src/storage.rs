use soroban_sdk::{contracttype, panic_with_error, unwrap::UnwrapOptimized, Address, Env, Symbol};

use crate::{errors::FeeVaultError, vault::VaultData};

//********** Storage Keys **********//

const POOL_KEY: &str = "Pool";
const ADMIN_KEY: &str = "Admin";
const ASSET_KEY: &str = "Asset";
const FEE_KEY: &str = "Fee";
const SIGNER_KEY: &str = "Signer";
const VAULT_DATA_KEY: &str = "Vault";
const REWARD_TOKEN_KEY: &str = "RwdToken";

#[derive(Clone)]
#[contracttype]
pub enum FeeVaultDataKey {
    Shares(Address),
    Rwd(Address),
    UserRwd(UserRewardKey),
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

/// The vault's reward data
#[derive(Clone)]
#[contracttype]
pub struct RewardData {
    // The expiration time of the rewards
    pub expiration: u64,
    // The earnings per share of the vault
    pub eps: u64,
    // The last time the rewards were updated
    pub last_time: u64,
    // The vault's reward index
    pub index: i128,
}

#[derive(Clone)]
#[contracttype]
pub struct UserRewardKey {
    pub user: Address,
    pub token: Address,
}

/// The user's reward data
#[derive(Clone)]
#[contracttype]
pub struct UserRewards {
    // The user's last reward index
    pub index: i128,
    // The user's accrued rewards
    pub accrued: i128,
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

/// Get the reward token address. Can be None if no reward has been set.
pub fn get_reward_token(e: &Env) -> Option<Address> {
    e.storage()
        .instance()
        .get::<Symbol, Address>(&Symbol::new(e, REWARD_TOKEN_KEY))
}

/// Set the reward token address
pub fn set_reward_token(e: &Env, token: &Address) {
    e.storage()
        .instance()
        .set::<Symbol, Address>(&Symbol::new(e, REWARD_TOKEN_KEY), token);
}

/********** Persistent **********/
// @dev
// Persistent data is not bumped on read, the data's access patterns mean they are almost always written
// when accessed, unless when used off-chain (e.g. by a frontend).

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
    e.storage()
        .persistent()
        .get::<Symbol, VaultData>(&key)
        .unwrap_or_else(|| panic_with_error!(e, FeeVaultError::ReserveNotFound))
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
    e.storage()
        .persistent()
        .get::<FeeVaultDataKey, i128>(&key)
        .unwrap_or(0)
}

/// Set the reward data
///
/// ### Arguments
/// * `token` - The address of the reward token
/// * `reward_data` - The rewards data
pub fn set_reward_data(e: &Env, token: &Address, reward_data: &RewardData) {
    let key = FeeVaultDataKey::Rwd(token.clone());
    e.storage()
        .persistent()
        .set::<FeeVaultDataKey, RewardData>(&key, reward_data);
    e.storage()
        .persistent()
        .extend_ttl(&key, LEDGER_THRESHOLD_USER, LEDGER_BUMP_USER);
}

/// Get the reward data
///
/// ### Arguments
/// * `token` - The address of the reward token
pub fn get_reward_data(e: &Env, token: &Address) -> Option<RewardData> {
    let key = FeeVaultDataKey::Rwd(token.clone());
    e.storage()
        .persistent()
        .get::<FeeVaultDataKey, RewardData>(&key)
}

/// Set the user's reward data
///
/// ### Arguments
/// * `user` - The address of the user
/// * `token` - The address of the reward token
/// * `user_data` - The user's reward data
pub fn set_user_rewards(e: &Env, user: &Address, token: &Address, user_data: &UserRewards) {
    let key = FeeVaultDataKey::UserRwd(UserRewardKey {
        token: token.clone(),
        user: user.clone(),
    });
    e.storage()
        .persistent()
        .set::<FeeVaultDataKey, UserRewards>(&key, user_data);
    e.storage()
        .persistent()
        .extend_ttl(&key, LEDGER_THRESHOLD_USER, LEDGER_BUMP_USER);
}

/// Get the reward data
///
/// ### Arguments
/// * `user` - The address of the user
/// * `token` - The address of the reward token
pub fn get_user_rewards(e: &Env, user: &Address, token: &Address) -> Option<UserRewards> {
    let key = FeeVaultDataKey::UserRwd(UserRewardKey {
        token: token.clone(),
        user: user.clone(),
    });
    e.storage()
        .persistent()
        .get::<FeeVaultDataKey, UserRewards>(&key)
}
