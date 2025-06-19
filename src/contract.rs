use crate::{
    errors::FeeVaultError,
    events::FeeVaultEvents,
    pool, storage,
    validator::{require_positive, require_valid_fee},
    vault::{self, VaultData},
};

use soroban_sdk::{contract, contractimpl, Address, Env, Vec};

#[contract]
pub struct FeeVault;

#[contractimpl]
impl FeeVault {
    /// Initialize the contract
    ///
    /// ### Arguments
    /// * `admin` - The admin address
    /// * `pool` - The blend pool address the vault will deposit into
    /// * `asset` - The asset address of the reserve the vault will support
    /// * `rate_type` - The rate type the vault will use
    ///     * 0 = take rate (admin earns a percentage of the vault's earnings)
    ///     * 1 = capped rate (vault earns at most the APR cap, with any additional returns going to the admin)
    ///     * 2 = fixed rate (vault always earns the fixed rate, with the admin either supplmenting or earning the difference)
    /// * `rate` - The rate value, with 7 decimals (e.g. 1000000 for 10%)
    /// * `signer`- The signer address if the vault is permissioned, None otherwise
    ///
    /// ### Panics
    /// * `InvalidFeeRate` - If the value is not within 0 and 1_000_0000
    /// * `InvalidFeeRateType` - If the rate type is not 0, 1, or 2
    pub fn __constructor(
        e: Env,
        admin: Address,
        pool: Address,
        asset: Address,
        rate_type: u32,
        rate: u32,
        signer: Option<Address>,
    ) {
        admin.require_auth();

        storage::set_admin(&e, admin);
        storage::set_pool(&e, pool.clone());
        storage::set_asset(&e, asset.clone());

        let fee = storage::Fee { rate_type, rate };
        require_valid_fee(&e, &fee);
        storage::set_fee(&e, fee);
        if let Some(signer) = signer {
            storage::set_signer(&e, signer);
        }
        storage::set_vault_data(
            &e,
            &VaultData {
                b_rate: pool::reserve_b_rate(&e, &pool, &asset),
                last_update_timestamp: e.ledger().timestamp(),
                total_shares: 0,
                total_b_tokens: 0,
                admin_balance: 0,
            },
        );
    }

    //********** Read-Only ***********//

    /// Fetch a user's position in shares
    ///
    /// ### Arguments
    /// * `user` - The address of the user
    ///
    /// ### Returns
    /// * `i128` - The user's position in shares, or the user has no shares
    pub fn get_shares(e: Env, user: Address) -> i128 {
        storage::get_vault_shares(&e, &user)
    }

    /// Fetch a user's position in bTokens
    ///
    /// ### Arguments
    /// * `user` - The address of the user
    ///
    /// ### Returns
    /// * `i128` - The user's position in bTokens, or 0 if they have no bTokens
    pub fn get_b_tokens(e: Env, user: Address) -> i128 {
        let shares = storage::get_vault_shares(&e, &user);
        if shares > 0 {
            let pool = storage::get_pool(&e);
            let asset = storage::get_asset(&e);
            let vault = vault::get_vault_updated(&e, &pool, &asset);
            vault.shares_to_b_tokens_down(shares)
        } else {
            0
        }
    }

    /// Fetch a user's position in underlying tokens
    ///
    /// ### Arguments
    /// * `user` - The address of the user
    ///
    /// ### Returns
    /// * `i128` - The user's position in underlying tokens, or 0 if they have no shares
    pub fn get_underlying_tokens(e: Env, user: Address) -> i128 {
        let shares = storage::get_vault_shares(&e, &user);
        if shares > 0 {
            let pool = storage::get_pool(&e);
            let asset = storage::get_asset(&e);
            let vault = vault::get_vault_updated(&e, &pool, &asset);
            let b_tokens = vault.shares_to_b_tokens_down(shares);
            vault.b_tokens_to_underlying_down(b_tokens)
        } else {
            0
        }
    }

    /// Fetch the accrued fees in underlying tokens
    ///
    /// ### Returns
    /// * `i128` - The admin's accrued fees in underlying tokens, or 0 if the reserve does not exist
    pub fn get_collected_fees(e: Env) -> i128 {
        let pool = storage::get_pool(&e);
        let asset = storage::get_asset(&e);
        let vault = vault::get_vault_updated(&e, &pool, &asset);
        vault.b_tokens_to_underlying_down(vault.admin_balance)
    }

    /// Get the blend pool address
    ///
    /// ### Returns
    /// * `Address` - The blend pool address
    pub fn get_pool(e: Env) -> Address {
        storage::get_pool(&e)
    }

    /// Get the vault data
    ///
    /// ### Returns
    /// * `VaultData` - The vault data
    pub fn get_vault(e: Env) -> VaultData {
        let pool = storage::get_pool(&e);
        let asset = storage::get_asset(&e);
        vault::get_vault_updated(&e, &pool, &asset)
    }

    //********** Read-Write Admin Only ***********//

    /// ADMIN ONLY
    /// Sets the Fee mode for the fee vault
    ///
    /// ### Arguments
    /// * `e` - The environment object
    /// * `is_apr_capped` - Whether the vault will be APR capped
    /// * `value` - The APR cap if `is_apr_capped`, the admin take_rate otherwise
    ///
    /// ### Panics
    /// * `InvalidFeeModeValue` - If the value is not within 0 and 1_000_0000
    pub fn set_fee(e: Env, rate_type: u32, rate: u32) {
        storage::extend_instance(&e);
        storage::get_admin(&e).require_auth();

        let fee = storage::Fee { rate_type, rate };
        require_valid_fee(&e, &fee);

        // Accrue interest prior to updating the fee-mode, to avoid any retroactive effect
        let pool = storage::get_pool(&e);
        let asset = storage::get_asset(&e);
        let vault = vault::get_vault_updated(&e, &pool, &asset);
        storage::set_vault_data(&e, &vault);

        storage::set_fee(&e, fee);

        FeeVaultEvents::fee_update(&e, rate_type, rate);
    }

    /// ADMIN ONLY
    /// Sets the admin address for the fee vault
    ///
    /// ### Arguments
    /// * `e` - The environment object
    /// * `admin` - The new admin address to set
    pub fn set_admin(e: Env, admin: Address) {
        storage::extend_instance(&e);
        storage::get_admin(&e).require_auth();
        admin.require_auth();
        storage::set_admin(&e, admin);
    }

    /// ADMIN ONLY
    /// Sets the signer for the fee vault. This address is required to sign
    /// all user deposits into the fee vault.
    ///
    /// ### Arguments
    /// * `e` - The environment object
    /// * `signer` - The new signer address to set
    pub fn set_signer(e: Env, signer: Address) {
        storage::extend_instance(&e);
        storage::get_admin(&e).require_auth();
        signer.require_auth();
        storage::set_signer(&e, signer);
    }

    /// ADMIN ONLY
    /// Claims emissions for the given reserves from the pool. This is a passthrough function
    /// that invokes the pool's "claim" function as the contract. More details can be found
    /// here: https://github.com/blend-capital/blend-contracts/blob/v1.0.0/pool/src/contract.rs#L192
    ///
    /// ### Arguments
    /// * `reserve_token_ids` - The ids of the reserves to claiming emissions for
    /// * `to` - The address to send the emissions to
    ///
    /// ### Returns
    /// * `i128` - The amount of blnd tokens claimed
    pub fn claim_emissions(e: Env, reserve_token_ids: Vec<u32>, to: Address) -> i128 {
        storage::extend_instance(&e);
        let admin = storage::get_admin(&e);
        admin.require_auth();
        let pool = storage::get_pool(&e);
        let emissions = pool::claim(&e, &pool, &reserve_token_ids, &to);

        FeeVaultEvents::vault_emissions_claim(&e, &pool, &admin, reserve_token_ids, emissions);
        emissions
    }

    /// ADMIN ONLY
    /// Deposit tokens into the valut in the admin balance
    ///
    /// ### Arguments
    /// * `amount` - The amount of tokens to deposit
    ///
    /// ### Returns
    /// * `i128` - The number of b_tokens minted
    ///
    /// ### Panics
    /// * `ReserveNotFound` - If the reserve does not have a vault
    /// * `InsufficientAccruedFees` - If there are no fees to claim
    pub fn admin_deposit(e: Env, amount: i128) -> i128 {
        storage::extend_instance(&e);
        let admin = storage::get_admin(&e);
        admin.require_auth();
        require_positive(&e, amount, FeeVaultError::InvalidAmount);

        let pool = storage::get_pool(&e);
        let asset = storage::get_asset(&e);
        pool::supply(&e, &pool, &asset, &admin, amount);
        let b_tokens_minted = vault::admin_deposit(&e, &pool, &asset, amount);

        FeeVaultEvents::vault_admin_deposit(&e, &pool, &asset, &admin, amount, b_tokens_minted);
        b_tokens_minted
    }

    /// ADMIN ONLY
    /// Withdraw tokens from the vault's admin balance
    ///
    /// ### Arguments
    /// * `amount` - The amount of underlying tokens to withdraw
    ///
    /// ### Returns
    /// * `i128` - The number of b_tokens burnt
    ///
    /// ### Panics
    /// * `InvalidAmount` - If the amount is less than or equal to 0
    /// * `BalanceError` - If the user does not have enough shares to withdraw the amount
    /// * `InvalidBTokensBurnt` - If the amount of bTokens burnt is less than or equal to 0
    pub fn admin_withdraw(e: Env, amount: i128) -> i128 {
        storage::extend_instance(&e);
        let admin = storage::get_admin(&e);
        admin.require_auth();
        require_positive(&e, amount, FeeVaultError::InvalidAmount);

        let pool = storage::get_pool(&e);
        let asset = storage::get_asset(&e);
        pool::withdraw(&e, &pool, &asset, &admin, amount);
        let b_tokens_burnt = vault::admin_withdraw(&e, &pool, &asset, amount);

        FeeVaultEvents::vault_admin_withdraw(&e, &pool, &asset, &admin, amount, b_tokens_burnt);
        b_tokens_burnt
    }

    //********** Read-Write ***********//

    /// Deposits tokens into the fee vault for a specific reserve. Requires the signer to sign
    /// the tranasction if the signer is set.
    ///
    /// ### Arguments
    /// * `user` - The address of the user making the deposit
    /// * `amount` - The amount of tokens to deposit
    ///
    /// ### Returns
    /// * `i128` - The number of shares minted for the user
    ///
    /// ### Panics
    /// * `ReserveNotFound` - If the reserve does not have a vault
    /// * `InvalidAmount` - If the amount is less than or equal to 0
    /// * `InvalidBTokensMinted` - If the amount of bTokens minted is less than or equal to 0
    /// * `InvalidSharesMinted` - If the amount of shares minted is less than or equal to 0
    pub fn deposit(e: Env, user: Address, amount: i128) -> i128 {
        storage::extend_instance(&e);
        user.require_auth();
        if let Some(signer) = storage::get_signer(&e) {
            signer.require_auth();
        }

        require_positive(&e, amount, FeeVaultError::InvalidAmount);

        let pool = storage::get_pool(&e);
        let asset = storage::get_asset(&e);
        pool::supply(&e, &pool, &asset, &user, amount);
        let (b_tokens_minted, new_shares) = vault::deposit(&e, &pool, &asset, &user, amount);

        FeeVaultEvents::vault_deposit(
            &e,
            &pool,
            &asset,
            &user,
            amount,
            new_shares,
            b_tokens_minted,
        );
        new_shares
    }

    /// Withdraws tokens from the fee vault for a specific reserve
    ///
    /// ### Arguments
    /// * `user` - The address of the user making the withdrawal
    /// * `amount` - The amount of tokens to withdraw
    ///
    /// ### Returns
    /// * `i128` - The number of shares burnt
    ///
    /// ### Panics
    /// * `ReserveNotFound` - If the reserve does not have a vault
    /// * `InvalidAmount` - If the amount is less than or equal to 0
    /// * `BalanceError` - If the user does not have enough shares to withdraw the amount
    /// * `InvalidBTokensBurnt` - If the amount of bTokens burnt is less than or equal to 0
    /// * `InsufficientReserves` - If the pool doesn't have enough reserves to complete the withdrawal
    pub fn withdraw(e: Env, user: Address, amount: i128) -> i128 {
        storage::extend_instance(&e);
        user.require_auth();
        require_positive(&e, amount, FeeVaultError::InvalidAmount);

        let pool = storage::get_pool(&e);
        let asset = storage::get_asset(&e);
        pool::withdraw(&e, &pool, &asset, &user, amount);
        let (b_tokens_burnt, burnt_shares) = vault::withdraw(&e, &pool, &asset, &user, amount);

        FeeVaultEvents::vault_withdraw(
            &e,
            &pool,
            &asset,
            &user,
            amount,
            burnt_shares,
            b_tokens_burnt,
        );
        burnt_shares
    }
}
