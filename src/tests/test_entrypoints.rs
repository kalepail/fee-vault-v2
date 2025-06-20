#![cfg(test)]

use crate::{
    constants::SCALAR_12,
    storage,
    testutils::{assert_approx_eq_rel, mockpool, register_fee_vault, EnvTestUtils},
    vault::VaultData,
    FeeVaultClient,
};
use soroban_fixed_point_math::FixedPoint;
use soroban_sdk::{
    testutils::{Address as _, AuthorizedFunction, AuthorizedInvocation},
    unwrap::UnwrapOptimized,
    vec, Address, Env, Error, IntoVal, Symbol,
};

#[test]
fn test_constructor_ok() {
    let e = Env::default();
    e.mock_all_auths();

    let samwise = Address::generate(&e);
    let frodo = Address::generate(&e);

    let init_b_rate = 1_000_000_000_000;
    let pool = mockpool::register_mock_pool_with_b_rate(&e, init_b_rate).address;
    let reserve = Address::generate(&e);
    let rate: u32 = 1_000_0000;
    let rate_type: u32 = 0;

    let vault_address = register_fee_vault(
        &e,
        &samwise,
        &pool,
        &reserve,
        rate_type,
        rate,
        Some(frodo.clone()),
    );

    assert_eq!(
        e.auths()[0],
        (
            samwise.clone(),
            AuthorizedInvocation {
                function: AuthorizedFunction::Contract((
                    vault_address.clone(),
                    Symbol::new(&e, "__constructor"),
                    vec![
                        &e,
                        samwise.into_val(&e),
                        pool.into_val(&e),
                        reserve.into_val(&e),
                        rate_type.into_val(&e),
                        rate.into_val(&e),
                        Some(frodo.clone()).into_val(&e),
                    ]
                )),
                sub_invocations: std::vec![]
            }
        )
    );

    let client = FeeVaultClient::new(&e, &vault_address);
    assert_eq!(client.get_config(), (pool, reserve));
    assert_eq!(client.get_admin(), samwise);
    assert_eq!(client.get_signer(), Some(frodo));
    let fee = client.get_fee();
    assert_eq!(fee.rate_type, rate_type);
    assert_eq!(fee.rate, rate);
    let vault_data = client.get_vault();
    assert_eq!(vault_data.total_b_tokens, 0);
    assert_eq!(vault_data.total_shares, 0);
    assert_eq!(vault_data.b_rate, init_b_rate);
    assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());
    assert_eq!(vault_data.admin_balance, 0);
}

#[test]
#[should_panic(expected = "Error(Context, InvalidAction)")]
fn test_constructor_invalid_rate() {
    let e = Env::default();
    e.mock_all_auths();

    let samwise = Address::generate(&e);
    let frodo = Address::generate(&e);

    let init_b_rate = 1_000_000_000_000;
    let pool = mockpool::register_mock_pool_with_b_rate(&e, init_b_rate).address;
    let reserve = Address::generate(&e);
    let rate: u32 = 1_0000000 + 1;
    let rate_type: u32 = 0;

    // Note: This fails with `InvalidAction` during testing, rather than `InvalidTakeRate`
    register_fee_vault(
        &e,
        &samwise,
        &pool,
        &reserve,
        rate_type,
        rate,
        Some(frodo.clone()),
    );
}

#[test]
#[should_panic(expected = "Error(Context, InvalidAction)")]
fn test_constructor_invalid_rate_type() {
    let e = Env::default();
    e.mock_all_auths();

    let samwise = Address::generate(&e);
    let frodo = Address::generate(&e);

    let init_b_rate = 1_000_000_000_000;
    let pool = mockpool::register_mock_pool_with_b_rate(&e, init_b_rate).address;
    let reserve = Address::generate(&e);
    let rate: u32 = 1_0000000 + 1;
    let rate_type: u32 = 3;

    // Note: This fails with `InvalidAction` during testing, rather than `InvalidTakeRate`
    register_fee_vault(
        &e,
        &samwise,
        &pool,
        &reserve,
        rate_type,
        rate,
        Some(frodo.clone()),
    );
}

#[test]
fn test_get_b_tokens() {
    let e = Env::default();
    e.mock_all_auths();
    e.set_default_info();

    let samwise = Address::generate(&e);
    let frodo = Address::generate(&e);

    let init_b_rate = 1_000_000_000_000;
    let pool = mockpool::register_mock_pool_with_b_rate(&e, init_b_rate).address;
    let reserve = Address::generate(&e);
    let rate: u32 = 100_0000;
    let rate_type: u32 = 0;

    let vault_address = register_fee_vault(&e, &samwise, &pool, &reserve, rate_type, rate, None);
    let vault_client = FeeVaultClient::new(&e, &vault_address);
    let mock_client = mockpool::MockPoolClient::new(&e, &pool);

    e.as_contract(&vault_address, || {
        let vault_data = VaultData {
            total_b_tokens: 1000_0000000,
            total_shares: 1200_0000000,
            b_rate: init_b_rate,
            last_update_timestamp: e.ledger().timestamp(),
            admin_balance: 0,
        };
        storage::set_vault_data(&e, &vault_data);

        // samwise owns 10% of the pool, frodo owns 90%
        storage::set_vault_shares(&e, &samwise, 120_0000000);
        storage::set_vault_shares(&e, &frodo, 1080_0000000);
    });
    assert_eq!(vault_client.get_b_tokens(&samwise), 100_0000000);
    assert_eq!(vault_client.get_b_tokens(&frodo), 900_0000000);

    // b_rate is increased by 10%. `take_rate` is 10%
    mock_client.set_b_rate(&1_100_000_000_000);
    e.jump(5);

    let expected_accrued_fees = 90909090_i128;
    let expected_total_b_tokens = 1000_0000000 - expected_accrued_fees;

    // Ensure get_b_tokens always returns updated results, even though b_rate hasn't been updated
    assert_eq!(
        vault_client.get_b_tokens(&samwise),
        expected_total_b_tokens
            .fixed_mul_floor(10, 100)
            .unwrap_optimized()
    );
    assert_eq!(
        vault_client.get_b_tokens(&frodo),
        expected_total_b_tokens
            .fixed_mul_floor(90, 100)
            .unwrap_optimized()
    );

    // The view function shouldn't mutate the state
    e.as_contract(&vault_address, || {
        let reserve_vault = storage::get_vault_data(&e);
        assert_eq!(reserve_vault.admin_balance, 0);
        assert_eq!(reserve_vault.total_b_tokens, 1000_0000000);
        assert_eq!(reserve_vault.total_shares, 1200_0000000);
        assert_eq!(reserve_vault.b_rate, 1_000_000_000_000);
    });

    // Should return 0 if user doesn't have any shares
    let non_existent_user = Address::generate(&e);
    assert_eq!(vault_client.get_b_tokens(&non_existent_user), 0);
}

#[test]
fn test_underlying_wrappers() {
    let e = Env::default();
    e.mock_all_auths();
    e.set_default_info();

    let samwise = Address::generate(&e);
    let frodo = Address::generate(&e);

    let init_b_rate = 1_000_000_000_000;
    let pool = mockpool::register_mock_pool_with_b_rate(&e, init_b_rate).address;
    let reserve = Address::generate(&e);
    let rate: u32 = 100_0000;
    let rate_type: u32 = 0;

    let vault_address = register_fee_vault(&e, &samwise, &pool, &reserve, rate_type, rate, None);
    let vault_client = FeeVaultClient::new(&e, &vault_address);
    let mock_client = mockpool::MockPoolClient::new(&e, &pool);

    e.as_contract(&vault_address, || {
        let vault_data = VaultData {
            total_b_tokens: 1000_0000000,
            total_shares: 1200_0000000,
            b_rate: init_b_rate,
            last_update_timestamp: e.ledger().timestamp(),
            admin_balance: 0,
        };
        storage::set_vault_data(&e, &vault_data);
        // samwise owns 10% of the pool, frodo owns 90%
        storage::set_vault_shares(&e, &samwise, 120_0000000);
        storage::set_vault_shares(&e, &frodo, 1080_0000000);
    });

    let total_underlying_value = init_b_rate * 1000_0000000 / SCALAR_12;
    let frodo_underlying = vault_client.get_underlying_tokens(&frodo);
    let samwise_underlying = vault_client.get_underlying_tokens(&samwise);

    // Since frodo owns 90% of the pool and sam owns 10%, we expect that
    // frodo's underlying value will be 9x sam's, and their sum will be the total.
    assert_eq!(
        frodo_underlying + samwise_underlying,
        total_underlying_value
    );
    assert_eq!(frodo_underlying, 9 * samwise_underlying);

    // There are no accrued fees initially
    assert_eq!(vault_client.get_underlying_admin_balance(), 0);

    // Assume b_rate is increased by 10%. The wrappers should take that into account
    mock_client.set_b_rate(&1_100_000_000_000);
    e.jump(5);

    // Since the growth is 10%, and the take_rate is also 10%,
    // the total accrued fees value should be `initial underlying / 100`.
    let accrued_fees_underlying = vault_client.get_underlying_admin_balance();
    assert_approx_eq_rel(
        accrued_fees_underlying,
        total_underlying_value / 100,
        0_0000001,
    );

    let sam_underlying_after = vault_client.get_underlying_tokens(&samwise);
    let frodo_underlying_after = vault_client.get_underlying_tokens(&frodo);

    // The new total underlying sum should be increased by 10%
    assert_approx_eq_rel(
        frodo_underlying_after + sam_underlying_after + accrued_fees_underlying,
        110 * total_underlying_value / 100,
        0_0000001,
    );

    // Both Frodo's and Sam's underlying value should've been increased by 9%
    assert_eq!(frodo_underlying_after, 109 * frodo_underlying / 100);
    assert_eq!(sam_underlying_after, 109 * samwise_underlying / 100);
    // Frodo's total underlying should still be 9x sam's
    assert_eq!(frodo_underlying_after, 9 * sam_underlying_after);

    // Ensure the view function never panic
    let non_existent_user = Address::generate(&e);
    assert_eq!(vault_client.get_underlying_tokens(&non_existent_user), 0);
}

#[test]
fn test_set_fee_mode() {
    let e = Env::default();
    e.mock_all_auths();

    let samwise = Address::generate(&e);

    let init_b_rate = 1_000_000_000_000;
    let pool = mockpool::register_mock_pool_with_b_rate(&e, init_b_rate).address;
    let reserve = Address::generate(&e);
    let rate: u32 = 1_000_0000;
    let rate_type: u32 = 0;

    let vault_address = register_fee_vault(&e, &samwise, &pool, &reserve, rate_type, rate, None);
    let vault_client = FeeVaultClient::new(&e, &vault_address);

    // value should be in range 0..1_000_0000
    assert_eq!(
        vault_client.try_set_fee(&0, &1_000_0001).err(),
        Some(Ok(Error::from_contract_error(104)))
    );

    // Set take rate to 0.5
    let take_rate = 500_000;
    vault_client.set_fee(&1, &take_rate);
    assert_eq!(
        e.auths()[0],
        (
            samwise.clone(),
            AuthorizedInvocation {
                function: AuthorizedFunction::Contract((
                    vault_address.clone(),
                    Symbol::new(&e, "set_fee"),
                    vec![&e, 1u32.into_val(&e), take_rate.into_val(&e),]
                )),
                sub_invocations: std::vec![]
            }
        )
    );
    e.as_contract(&vault_address, || {
        let fee = storage::get_fee(&e);
        assert_eq!(fee.rate_type, 1);
        assert_eq!(fee.rate, take_rate);
    });
    // Setting the value to 0 or 100% should be possible
    vault_client.set_fee(&0, &0);
    e.as_contract(&vault_address, || {
        let fee = storage::get_fee(&e);
        assert_eq!(fee.rate_type, 0);
        assert_eq!(fee.rate, 0);
    });

    vault_client.set_fee(&1, &1_000_0000);
    e.as_contract(&vault_address, || {
        let fee = storage::get_fee(&e);
        assert_eq!(fee.rate_type, 1);
        assert_eq!(fee.rate, 1_000_0000);
    });
}

#[test]
fn test_ensure_b_rate_gets_update_pre_fee_mode_update() {
    let e = Env::default();
    e.mock_all_auths();
    e.set_default_info();

    let samwise = Address::generate(&e);
    let frodo = Address::generate(&e);

    let init_b_rate = 1_000_000_000_000;
    let pool = mockpool::register_mock_pool_with_b_rate(&e, init_b_rate).address;
    let reserve = Address::generate(&e);
    let rate: u32 = 100_0000;
    let rate_type: u32 = 0;

    let vault_address = register_fee_vault(&e, &frodo, &pool, &reserve, rate_type, rate, None);
    let vault_client = FeeVaultClient::new(&e, &vault_address);
    let mock_client = mockpool::MockPoolClient::new(&e, &pool);

    e.as_contract(&vault_address, || {
        storage::set_vault_data(
            &e,
            &VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                last_update_timestamp: e.ledger().timestamp(),
                admin_balance: 0,
            },
        );

        // All the shares are owned by samwise for simplicity
        storage::set_vault_shares(&e, &samwise, 1200_0000000);
    });

    let usdc_underlying_balance_before = vault_client.get_underlying_tokens(&samwise);

    // The pool has doubled in value, but interest hasn't been accrued yet
    let new_b_rate = 2_000_000_000_000;
    mock_client.set_b_rate(&new_b_rate);
    e.jump(5);

    // Ensure everything is still equal to the initial config pre fee-mode update
    e.as_contract(&vault_address, || {
        let vault = storage::get_vault_data(&e);
        assert_eq!(vault.admin_balance, 0);
        assert_eq!(vault.b_rate, 1_000_000_000_000);
        assert_ne!(vault.last_update_timestamp, e.ledger().timestamp());
    });

    // Admin tries to take advantage of that by setting the take_rate to 100% to claim all the fees.
    vault_client.set_fee(&0, &1_000_0000);

    // The previous action shouldn't affect any already accrued rewards
    let usdc_underlying_balance_after = vault_client.get_underlying_tokens(&samwise);

    // The b_rate has doubled and the take_rate was 10%. So we expect 190% increase
    assert_eq!(
        usdc_underlying_balance_after,
        usdc_underlying_balance_before * 19 / 10
    );

    // Ensure the stored reserve vaults are also up to date
    e.as_contract(&vault_address, || {
        let usdc_vault = storage::get_vault_data(&e);
        assert_eq!(usdc_vault.admin_balance, 500000000);
        assert_eq!(usdc_vault.b_rate, new_b_rate);
        assert_eq!(usdc_vault.last_update_timestamp, e.ledger().timestamp());
        assert_eq!(usdc_vault.total_b_tokens, 1000_0000000 - 500000000);
    });
}

#[test]
fn test_set_admin() {
    let e = Env::default();
    e.mock_all_auths();

    let samwise = Address::generate(&e);
    let frodo = Address::generate(&e);

    let init_b_rate = 1_000_000_000_000;
    let pool = mockpool::register_mock_pool_with_b_rate(&e, init_b_rate).address;
    let reserve = Address::generate(&e);
    let rate: u32 = 1_000_0000;
    let rate_type: u32 = 0;

    let vault_address = register_fee_vault(&e, &samwise, &pool, &reserve, rate_type, rate, None);
    let vault_client = FeeVaultClient::new(&e, &vault_address);

    e.as_contract(&vault_address, || {
        // samwise is the current admin
        assert_eq!(storage::get_admin(&e), samwise.clone());
    });

    vault_client.set_admin(&frodo);

    let authorized_function = AuthorizedInvocation {
        function: AuthorizedFunction::Contract((
            vault_address.clone(),
            Symbol::new(&e, "set_admin"),
            vec![&e, frodo.into_val(&e)],
        )),
        sub_invocations: std::vec![],
    };
    // auths[0] should be the old admin, auths[1] should be the new admin
    assert_eq!(
        e.auths(),
        std::vec![
            (samwise.clone(), authorized_function.clone()),
            (frodo.clone(), authorized_function)
        ]
    );

    e.as_contract(&vault_address, || {
        // The new admin is frodo
        assert_eq!(storage::get_admin(&e), frodo);
    });

    // Frodo should be able to also set a new admin
    let new_admin = Address::generate(&e);
    vault_client.set_admin(&new_admin);

    let new_authorized_function = AuthorizedInvocation {
        function: AuthorizedFunction::Contract((
            vault_address.clone(),
            Symbol::new(&e, "set_admin"),
            vec![&e, new_admin.into_val(&e)],
        )),
        sub_invocations: std::vec![],
    };
    assert_eq!(
        e.auths(),
        std::vec![
            (frodo.clone(), new_authorized_function.clone()),
            (new_admin.clone(), new_authorized_function)
        ]
    );
}

#[test]
fn test_set_signer() {
    let e = Env::default();
    e.mock_all_auths();

    let samwise = Address::generate(&e);
    let frodo = Address::generate(&e);
    let merry = Address::generate(&e);

    let init_b_rate = 1_000_000_000_000;
    let pool = mockpool::register_mock_pool_with_b_rate(&e, init_b_rate).address;
    let reserve = Address::generate(&e);
    let rate: u32 = 1_000_0000;
    let rate_type: u32 = 0;

    let vault_address = register_fee_vault(
        &e,
        &samwise,
        &pool,
        &reserve,
        rate_type,
        rate,
        Some(merry.clone()),
    );
    let vault_client = FeeVaultClient::new(&e, &vault_address);

    e.as_contract(&vault_address, || {
        // merry is the current signer
        assert_eq!(storage::get_signer(&e), Some(merry.clone()));
    });

    vault_client.set_signer(&frodo);

    let authorized_function = AuthorizedInvocation {
        function: AuthorizedFunction::Contract((
            vault_address.clone(),
            Symbol::new(&e, "set_signer"),
            vec![&e, frodo.into_val(&e)],
        )),
        sub_invocations: std::vec![],
    };
    // auths[0] should be the admin, auths[1] should be the new signer
    assert_eq!(
        e.auths(),
        std::vec![
            (samwise.clone(), authorized_function.clone()),
            (frodo.clone(), authorized_function)
        ]
    );

    e.as_contract(&vault_address, || {
        // The new signer is frodo
        assert_eq!(storage::get_signer(&e), Some(frodo.clone()));
    });
}
