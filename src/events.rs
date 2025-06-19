use soroban_sdk::{Address, Env, Symbol, Vec};

pub struct FeeVaultEvents {}

impl FeeVaultEvents {
    /// Emitted when a deposit is performed against the vault
    ///
    /// - topics - `["vault_deposit", pool: Address, reserve: Address, from: Address]`
    /// - data - `[amount: i128, shares: i128, b_tokens: i128]`
    pub fn vault_deposit(
        e: &Env,
        pool: &Address,
        reserve: &Address,
        from: &Address,
        amount: i128,
        shares: i128,
        b_tokens: i128,
    ) {
        let topics = (
            Symbol::new(&e, "vault_deposit"),
            pool.clone(),
            reserve.clone(),
            from.clone(),
        );
        e.events().publish(topics, (amount, shares, b_tokens));
    }

    /// Emitted when a withdraw is performed against the vault
    ///
    /// - topics - `["vault_withdraw", pool: Address, reserve: Address, from: Address]`
    /// - data - `[amount: i128, shares: i128, b_tokens: i128]`
    pub fn vault_withdraw(
        e: &Env,
        pool: &Address,
        reserve: &Address,
        from: &Address,
        amount: i128,
        shares: i128,
        b_tokens: i128,
    ) {
        let topics = (
            Symbol::new(&e, "vault_withdraw"),
            pool.clone(),
            reserve.clone(),
            from.clone(),
        );
        e.events().publish(topics, (amount, shares, b_tokens));
    }

    /// Emitted when the admin adds b_tokens to the vault
    ///
    /// - topics - `["vault_admin_deposit", pool: Address, reserve: Address, admin: Address]`
    /// - data - `[amount: i128, b_tokens: i128]`
    pub fn vault_admin_deposit(
        e: &Env,
        pool: &Address,
        reserve: &Address,
        admin: &Address,
        amount: i128,
        b_tokens: i128,
    ) {
        let topics = (
            Symbol::new(&e, "vault_admin_deposit"),
            pool.clone(),
            reserve.clone(),
            admin.clone(),
        );
        e.events().publish(topics, (amount, b_tokens));
    }

    /// Emitted when the admin withdraws b_tokens from the vault
    ///
    /// - topics - `["vault_admin_withdraw", pool: Address, reserve: Address, admin: Address]`
    /// - data - `[amount: i128, b_tokens: i128]`
    pub fn vault_admin_withdraw(
        e: &Env,
        pool: &Address,
        reserve: &Address,
        admin: &Address,
        amount: i128,
        b_tokens: i128,
    ) {
        let topics = (
            Symbol::new(&e, "vault_admin_deposit"),
            pool.clone(),
            reserve.clone(),
            admin.clone(),
        );
        e.events().publish(topics, (amount, b_tokens));
    }

    /// Emitted when emissions are claimed
    ///
    /// - topics - `["vault_emissions_claim", admin: Address]`
    /// - data - `amount: i128`
    pub fn vault_emissions_claim(
        e: &Env,
        admin: &Address,
        pool: &Address,
        reserve_token_ids: Vec<u32>,
        amount: i128,
    ) {
        let topics = (
            Symbol::new(&e, "vault_emissions_claim"),
            pool.clone(),
            admin.clone(),
        );
        e.events().publish(topics, (reserve_token_ids, amount));
    }

    /// Emitted when the fee config is updated for the fee vault
    ///
    /// - topics - `["fee_update"]`
    /// - data - `[rate_type: u32, rate: u32]`
    pub fn fee_update(e: &Env, rate_type: u32, rate: u32) {
        let topics = (Symbol::new(&e, "fee_update"),);

        e.events().publish(topics, (rate_type, rate));
    }
}
