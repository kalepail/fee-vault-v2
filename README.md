# Overview

This is a fee vault for Blend pools. It is used to allow an admin to collect a portion of the interest earned from Blend pools by the vault depositors along with all emissions accrued by vault depositors. It can also be used to supplement user deposits or add additional token rewards. Wallets and integrating protocols are the entities typically interested in this functionality.

# How it works

The fee vault contract interacts with the underlying blend pool on behalf of users. It acts as a ERC4626-like token vault contract, where the vault holds a given asset's `b_tokens`, and issues `shares` to depositors that represent ownership of the vaults `b_tokens`. For more information about how token vault accounting works, please see: https://eips.ethereum.org/EIPS/eip-4626.

The fee vault can be permissioned with the use of a `signer`. If this parameter is set, no user will be able to enter the fee vault unless the `signer` has signed the transaction. Withdrawing from the fee vault does not require a `signer` signature.

The fee vault can be setup in three different configurations: take rate, capped rate, and fixed rate.

Regardless of the configuration, admins can manage their balance of `b_tokens` at any time.

### Take Rate

A take rate fee vault will take a portion of all gains earned by the vault as a fee for the admin on each accrual period. That is, if the vault earns 100 tokens over a day and the vault has a 10% take rate, the users will earn 90 tokens and the admin will earn 10 tokens.

The fees are taken from the vaults `b_tokens` by calculating how many `b_tokens` the 10 tokens is worth. This way, fees are taken fairly from all users in the vault.

### Capped Rate

A capped rate fee vault will only take gains past the vaults provided rate as a fee for the admin. The vault calculates the interest rate between the current vault interaction and the last time the vault was interacted with. If the interest rate exceeds the capped rate, the excess gains are calculated such that users will earn the capped rate. The excess gains are converted to `b_tokens` and are taken from the vault as an admin fee.

If the calculated interest earned over the update period is below the capped rate, no fees are taken.

### Fixed Rate

A fixed rate fee vault works the same as a capped rate fee vault when the interest rate is above the fixed rate, but will attempt to supplement the users gains if the calculated interest rate over the interaction period is below the fixed rate.

If the admin does not maintain a positive `admin_balance`, the vault users will not be supplemented. That is, a fixed rate fee vault will only supplement users yield with existing `b_tokens` in the `admin_balance`.

# Usage

## Setup

To set up a fee vault for a blend pool, the admin must first deploy a new fee vault contract.

The contracts are initialized through the `__constructor`.

```rust
    /// Initialize the contract
    ///
    /// ### Arguments
    /// * `admin` - The admin address
    /// * `pool` - The blend pool address the vault will deposit into
    /// * `asset` - The asset address of the reserve the vault will support
    /// * `rate_type` - The rate type the vault will use
    ///     * 0 = take rate (admin earns a percentage of the vault's earnings)
    ///     * 1 = capped rate (vault earns at most the APR cap, with any additional returns going to the admin)
    ///     * 2 = fixed rate (vault always earns the fixed rate, with the admin either supplementing or earning the difference)
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
    )
```

## Integration

To integrate the fee vault into your app or protocol, you will just need to have users deposit with the vaults `deposit` function. If there is a `signer`, that address will also need to sign the transaction.

```rust
    /// Deposits tokens into the fee vault for a specific reserve. Requires the signer to sign
    /// the transaction if the signer is set.
    ///
    /// ### Arguments
    /// * `user` - The address of the user making the deposit
    /// * `amount` - The amount of tokens to deposit
    ///
    /// ### Returns
    /// * `i128` - The number of shares minted for the user
    ///
    /// ### Panics
    /// * `InvalidAmount` - If the amount is less than or equal to 0
    /// * `InvalidBTokensMinted` - If the amount of bTokens minted is less than or equal to 0
    /// * `InvalidSharesMinted` - If the amount of shares minted is less than or equal to 0
    /// * `BalanceError` - If the user does not have enough tokens
    pub fn deposit(e: Env, user: Address, amount: i128) -> i128
```

and withdraw using the `withdraw` function.

```rust
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
    /// * `InvalidAmount` - If the amount is less than or equal to 0
    /// * `BalanceError` - If the user does not have enough shares to withdraw the amount
    /// * `InvalidBTokensBurnt` - If the amount of bTokens burnt is less than or equal to 0
    /// * `InsufficientReserves` - If the pool doesn't have enough reserves to complete the withdrawal
    pub fn withdraw(e: Env, user: Address, amount: i128) -> i128
```

You can display to users their current underlying asset balance using the `get_underlying_tokens` function.

```rust
    /// Fetch a user's position in underlying tokens
    ///
    /// ### Arguments
    /// * `user` - The address of the user
    ///
    /// ### Returns
    /// * `i128` - The user's position in underlying tokens, or 0 if they have no shares
    pub fn get_underlying_tokens(e: Env, user: Address) -> i128
```

You can display general vault information, including the Blend pool, supported asset, rewards info, and the estimated APR users will earn using the `get_vault_summary` function.

```rust
    /// NOT INTENDED FOR CONTRACT USE
    ///
    /// Get the vault summary, which includes the pool, asset, admin, signer, fee, vault data,
    /// and estimated APR for vault suppliers. Intended for use by dApps looking to fetch
    /// display data.
    ///
    /// ### Returns
    /// * `VaultSummary` - The summary of the vault
    pub fn get_vault_summary(e: Env) -> VaultSummary
```

## Rewards

The fee vault contains the ability to add rewards for the users depositing into the fee vault. All rewards are issued based on vault `shares` held over time, and are distributed equally to all vault `share` holders.

To setup rewards, the admin can invoke the `set_rewards` function. Note that if there is an active reward period ongoing, the reward token cannot be changed.

```rust
    /// ADMIN ONLY
    /// Sets rewards to be distributed to the fee vault depositors. The full `reward_amount` will be
    /// transferred to the vault to be distributed to the users until the `expiration` timestamp.
    ///
    /// ### Arguments
    /// * `token` - The address of the reward token
    /// * `reward_amount` - The amount of rewards to distribute
    /// * `expiration` - The timestamp when the rewards expire
    ///
    /// ### Panics
    /// * `InvalidRewardConfig` - If the reward token cannot be changed, or if a valid reward period cannot be started
    /// * `BalanceError` - If the admin does not have enough tokens to set the rewards
    pub fn set_rewards(e: Env, token: Address, reward_amount: i128, expiration: u64)
```

To view the current reward token, use the `get_reward_token` function, or see `Integrations` as the data is included in the `VaultSummary` object. 

```rust
    /// Get the current reward token for the fee vault
    ///
    /// ### Returns
    /// * `Option<Address>` - The address of the reward token, or None if no reward token is set
    pub fn get_reward_token(e: Env) -> Option<Address>
```

To view the current reward token's data, use the `get_reward_data` function, or see `Integrations` as the data is included in the `VaultSummary` object. 

```rust
    /// Get the reward data for a specific token
    ///
    /// ### Arguments
    /// * `token` - The address of the reward token
    ///
    /// ### Returns
    /// * `Option<RewardData>` - The reward data for the token, or None if no data exists
    pub fn get_reward_data(e: Env, token: Address) -> Option<RewardData> 
```

To claim the rewards for a user, use the `claim_rewards` function. For dApps, it is recommended to simulate this function to determine the current rewards a user has available to claim.

```rust
    /// Claims rewards for the user from the fee vault.
    ///
    /// ### Arguments
    /// * `user` - The address of the user claiming rewards
    /// * `reward_token` - The address of the reward token to claim
    /// * `to` - The address to send the claimed rewards to
    ///
    /// ### Returns
    /// * `i128` - The amount of rewards claimed
    ///
    /// ### Panics
    /// * `NoRewardsConfigured` - If no rewards are configured for the token
    pub fn claim_rewards(e: Env, user: Address, reward_token: Address, to: Address) -> i128 
```

## Admin Balance Management

Admins can withdraw or deposit funds into their balance pool. Fees will be added to their balance over time based on the fee vaults configuration.

To withdraw funds from the admin balance, use the `admin_withdraw` function.

```rust
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
    pub fn admin_withdraw(e: Env, amount: i128) -> i128
```

To deposit additional funds into the admin balance, often used when the vault is using a fixed rate fee, use the `admin_deposit` function.

```rust
    /// ADMIN ONLY
    /// Deposit tokens into the vault's admin balance
    ///
    /// ### Arguments
    /// * `amount` - The amount of tokens to deposit
    ///
    /// ### Returns
    /// * `i128` - The number of b_tokens minted
    ///
    /// ### Panics
    /// * `InvalidAmount` - If the amount is less than or equal to 0
    /// * `InvalidBTokensMinted` - If the amount of bTokens minted is less than or equal to 0
    /// * `BalanceError` - If the user does not have enough tokens
    pub fn admin_deposit(e: Env, amount: i128) -> i128
```

To fetch the underlying asset balance the admin has, use the `get_underlying_admin_balance` function.

```rust
    /// Fetch the admin balance in underlying tokens
    ///
    /// ### Returns
    /// * `i128` - The admin's accrued fees in underlying tokens, or 0 if the reserve does not exist
    pub fn get_underlying_admin_balance(e: Env) -> i128
```

# Limitations

## Collateralizing and Borrowing

The fee vault contract does not currently support collateralizing and borrowing. It only supplies and withdraws tokens from the blend pool.

# Other notes

## Inflation Attacks

The vault is safe against inflation attacks as it relies on internally tracked supply rather than token balances.
