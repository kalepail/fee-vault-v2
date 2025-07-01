#![cfg(test)]

use crate::testutils::{assert_approx_eq_abs, create_blend_pool, register_fee_vault, EnvTestUtils};
use crate::FeeVaultClient;
use blend_contract_sdk::pool::{Client as PoolClient, Request};
use blend_contract_sdk::testutils::BlendFixture;
use sep_41_token::testutils::MockTokenClient;
use soroban_sdk::testutils::{Address as _, AuthorizedFunction, AuthorizedInvocation};
use soroban_sdk::{vec, Address, Env, IntoVal, Symbol};

#[test]
fn test_rewards() {
    let e = Env::default();
    e.cost_estimate().budget().reset_unlimited();
    e.mock_all_auths();
    e.set_default_info();

    let bombadil = Address::generate(&e);
    let gandalf = Address::generate(&e);
    let frodo = Address::generate(&e);
    let samwise = Address::generate(&e);
    let merry = Address::generate(&e);

    let blnd = e
        .register_stellar_asset_contract_v2(bombadil.clone())
        .address();
    let usdc = e
        .register_stellar_asset_contract_v2(bombadil.clone())
        .address();
    let xlm = e
        .register_stellar_asset_contract_v2(bombadil.clone())
        .address();
    let blnd_client = MockTokenClient::new(&e, &blnd);
    let usdc_client = MockTokenClient::new(&e, &usdc);
    let xlm_client = MockTokenClient::new(&e, &xlm);

    // create blend pool and fee vault
    let blend_fixture = BlendFixture::deploy(&e, &bombadil, &blnd, &usdc);
    let pool = create_blend_pool(&e, &blend_fixture, &bombadil, &usdc_client, &xlm_client);
    let pool_client = PoolClient::new(&e, &pool);

    let fee_vault = register_fee_vault(&e, &gandalf, &pool, &usdc, 0, 100_0000, None);
    let fee_vault_client = FeeVaultClient::new(&e, &fee_vault);

    // Setup pool util rate
    // Bomadil deposits 200k tokens and borrows 100k tokens for a 50% util rate
    let requests = vec![
        &e,
        Request {
            address: usdc.clone(),
            amount: 200_000_0000000,
            request_type: 2,
        },
        Request {
            address: usdc.clone(),
            amount: 100_000_0000000,
            request_type: 4,
        },
        Request {
            address: xlm.clone(),
            amount: 200_000_0000000,
            request_type: 2,
        },
        Request {
            address: xlm.clone(),
            amount: 100_000_0000000,
            request_type: 4,
        },
    ];
    pool_client
        .mock_all_auths()
        .submit(&bombadil, &bombadil, &bombadil, &requests);

    /*
     * Test XLM rewards for USDC vault
     */

    // -> create initial deposit for Frodo
    let frodo_deposit = 100_0000000;
    usdc_client.mint(&frodo, &frodo_deposit);
    fee_vault_client.deposit(&frodo, &frodo_deposit);

    e.jump_time(1000);

    // -> setup XLM rewards w/ fee vault admin (gandalf)
    let xlm_rewards: i128 = 12_345_0000000;
    let xlm_reward_period: u64 = 100_000;
    xlm_client.mint(&gandalf, &xlm_rewards);

    fee_vault_client.set_rewards(
        &xlm,
        &xlm_rewards,
        &(e.ledger().timestamp() + xlm_reward_period),
    );
    // -> verify set_rewards
    assert_eq!(
        e.auths()[0],
        (
            gandalf.clone(),
            AuthorizedInvocation {
                function: AuthorizedFunction::Contract((
                    fee_vault.clone(),
                    Symbol::new(&e, "set_rewards"),
                    vec![
                        &e,
                        xlm.to_val(),
                        xlm_rewards.into_val(&e),
                        (e.ledger().timestamp() + xlm_reward_period).into_val(&e),
                    ]
                )),
                sub_invocations: std::vec![AuthorizedInvocation {
                    function: AuthorizedFunction::Contract((
                        xlm.clone(),
                        Symbol::new(&e, "transfer"),
                        vec![
                            &e,
                            gandalf.to_val(),
                            fee_vault.to_val(),
                            xlm_rewards.into_val(&e),
                        ]
                    )),
                    sub_invocations: std::vec![]
                }]
            }
        )
    );
    let reward_token_result = fee_vault_client.get_reward_token().unwrap();
    assert_eq!(reward_token_result, xlm);
    let reward_data = fee_vault_client.get_reward_data(&xlm).unwrap();
    assert_eq!(reward_data.index, 0);
    assert_eq!(reward_data.last_time, e.ledger().timestamp());
    assert_eq!(
        reward_data.expiration,
        e.ledger().timestamp() + xlm_reward_period
    );
    assert_eq!(reward_data.eps, 0_1234500);
    assert_eq!(xlm_client.balance(&fee_vault), xlm_rewards);
    assert_eq!(xlm_client.balance(&gandalf), 0);

    // -> skip half the reward period
    e.jump_time(xlm_reward_period / 2);

    // -> validate `get_vault_summary` and `get_reward_data` return updated values for the rewards
    let vault_summary = fee_vault_client.get_vault_summary();
    let updated_reward_data = fee_vault_client.get_reward_data(&xlm).unwrap();
    assert_eq!(vault_summary.reward_token, Some(xlm.clone()));
    assert_eq!(vault_summary.reward_data.eps, reward_data.eps);
    assert!(vault_summary.reward_data.index > 0);
    assert_eq!(vault_summary.reward_data.last_time, e.ledger().timestamp());
    assert_eq!(vault_summary.reward_data.expiration, reward_data.expiration);

    assert_eq!(updated_reward_data.eps, reward_data.eps);
    assert!(updated_reward_data.index > 0);
    assert_eq!(updated_reward_data.last_time, e.ledger().timestamp());
    assert_eq!(updated_reward_data.expiration, reward_data.expiration);

    assert_eq!(updated_reward_data.index, vault_summary.reward_data.index);

    // -> samwise deposits ~200 USDC into the fee vault
    // -> use double frodo's balance to remove interest rate effects
    let samwise_deposit = fee_vault_client.get_underlying_tokens(&frodo) * 2;
    usdc_client.mint(&samwise, &samwise_deposit);
    fee_vault_client.deposit(&samwise, &samwise_deposit);

    // -> skip half the reward period
    e.jump_time(xlm_reward_period / 2);

    // -> frodo and samwise claim rewards. Samwise claims rewards to merry.
    //    Frodo should earn 100% of the first half and 33.33% of the second half, or 66.66% of total rewards
    //    Samwise should earn 0% of the first half, and 66.66% of the second half, or 33.33% of total rewards
    let frodo_xlm_balance_0 = xlm_client.balance(&frodo);
    let samwise_xlm_balance_0 = xlm_client.balance(&samwise);
    let merry_xlm_balance_0 = xlm_client.balance(&merry);
    let frodo_claimed = fee_vault_client.claim_rewards(&frodo, &xlm, &frodo);
    let samwise_claimed = fee_vault_client.claim_rewards(&samwise, &xlm, &merry);

    let frodo_xlm_balance_1 = xlm_client.balance(&frodo);
    let samwise_xlm_balance_1 = xlm_client.balance(&samwise);
    let merry_xlm_balance_1 = xlm_client.balance(&merry);
    assert_approx_eq_abs(frodo_claimed, 8230_0000000, 0_0000100);
    assert_approx_eq_abs(samwise_claimed, 4115_0000000, 0_0000100);
    assert_eq!(frodo_xlm_balance_1, frodo_xlm_balance_0 + frodo_claimed);
    assert_eq!(samwise_xlm_balance_1, samwise_xlm_balance_0);
    assert_eq!(merry_xlm_balance_1, merry_xlm_balance_0 + samwise_claimed);
    // some rounding loss expected
    assert_eq!(
        xlm_client.balance(&fee_vault),
        xlm_rewards - (frodo_claimed + samwise_claimed)
    );

    // -> Frodo fully withdraw from the vault, and validate rewards stopped
    let xlm_reward_data_post_claim = fee_vault_client.get_reward_data(&xlm).unwrap();
    e.jump_time(10);
    let frodo_underlying_balance = fee_vault_client.get_underlying_tokens(&frodo);
    fee_vault_client.withdraw(&frodo, &frodo_underlying_balance);
    let xlm_reward_data_post_jump = fee_vault_client.get_reward_data(&xlm).unwrap();
    assert_eq!(
        xlm_reward_data_post_claim.index,
        xlm_reward_data_post_jump.index
    );

    /*
     * Test changing the reward token to BLND and processing new rewards
     */

    // -> set BLND rewards w/ fee vault admin (gandalf)
    let blnd_rewards: i128 = 1_000_0000000;
    let blnd_reward_period: u64 = 100_000;
    blnd_client.mint(&gandalf, &blnd_rewards);
    fee_vault_client.set_rewards(
        &blnd,
        &blnd_rewards,
        &(e.ledger().timestamp() + blnd_reward_period),
    );
    let reward_token_result = fee_vault_client.get_reward_token().unwrap();
    assert_eq!(reward_token_result, blnd);
    let reward_data = fee_vault_client.get_reward_data(&blnd).unwrap();
    assert_eq!(reward_data.index, 0);
    assert_eq!(reward_data.last_time, e.ledger().timestamp());
    assert_eq!(
        reward_data.expiration,
        e.ledger().timestamp() + blnd_reward_period
    );
    assert_eq!(reward_data.eps, 0_0100000);
    assert_eq!(blnd_client.balance(&fee_vault), blnd_rewards);
    assert_eq!(blnd_client.balance(&gandalf), 0);

    // -> frodo deposits 3x the amount of samwise's deposit to have 3/4 of the shares
    let frodo_deposit = fee_vault_client.get_underlying_tokens(&samwise) * 3;
    usdc_client.mint(&frodo, &frodo_deposit);
    fee_vault_client.deposit(&frodo, &frodo_deposit);

    // -> have samwise and frodo touch their positions every 1/10 period
    // -> have frodo claim at the halfway point
    let frodo_blnd_balance_0 = blnd_client.balance(&frodo);
    let samwise_blnd_balance_0 = blnd_client.balance(&samwise);
    let merry_blnd_balance_0 = blnd_client.balance(&merry);
    let mut frodo_claimed_2 = 0;
    for i in 0..10 {
        e.jump_time(blnd_reward_period / 10);
        let samwise_temp_withdraw = 10;
        let frodo_temp_withdraw = 30;
        fee_vault_client.withdraw(&samwise, &samwise_temp_withdraw);
        fee_vault_client.withdraw(&frodo, &frodo_temp_withdraw);
        if i == 4 {
            frodo_claimed_2 = fee_vault_client.claim_rewards(&frodo, &blnd, &frodo);
        }
    }

    // -> claim rewards for samwise and frodo
    //    Merry should not earn any rewards, as he did not deposit
    //    Frodo should earn 3/4 of rewards, samwise should earn 1/4 of rewards

    frodo_claimed_2 += fee_vault_client.claim_rewards(&frodo, &blnd, &frodo);
    let samwise_claimed_2 = fee_vault_client.claim_rewards(&samwise, &blnd, &merry);
    let frodo_blnd_balance_1 = blnd_client.balance(&frodo);
    let samwise_blnd_balance_1 = blnd_client.balance(&samwise);
    let merry_blnd_balance_1 = blnd_client.balance(&merry);

    // note - a bit more rounding loss due to withdraws
    assert_approx_eq_abs(frodo_claimed_2, 750_0000000, 0_0001000);
    assert_approx_eq_abs(samwise_claimed_2, 250_0000000, 0_0001000);
    assert_eq!(frodo_blnd_balance_1, frodo_blnd_balance_0 + frodo_claimed_2);
    assert_eq!(samwise_blnd_balance_1, samwise_blnd_balance_0);
    assert_eq!(
        merry_blnd_balance_1,
        merry_blnd_balance_0 + samwise_claimed_2
    );
    assert_eq!(
        blnd_client.balance(&fee_vault),
        blnd_rewards - (frodo_claimed_2 + samwise_claimed_2)
    );

    /*
     * Test re-starting XLM rewards and boosting them
     */

    // -> setup XLM rewards w/ fee vault admin (gandalf)
    let xlm_rewards_0: i128 = 20_000_0000000;
    let xlm_reward_period: u64 = 100_000;
    xlm_client.mint(&gandalf, &xlm_rewards_0);

    let xlm_balance_vault_0 = xlm_client.balance(&fee_vault);
    let xlm_balance_gandalf_0 = xlm_client.balance(&gandalf);
    fee_vault_client.set_rewards(
        &xlm,
        &xlm_rewards_0,
        &(e.ledger().timestamp() + xlm_reward_period),
    );
    let reward_token_result = fee_vault_client.get_reward_token().unwrap();
    assert_eq!(reward_token_result, xlm);
    let reward_data = fee_vault_client.get_reward_data(&xlm).unwrap();
    assert_eq!(reward_data.index, xlm_reward_data_post_claim.index);
    assert_eq!(reward_data.last_time, e.ledger().timestamp());
    assert_eq!(
        reward_data.expiration,
        e.ledger().timestamp() + xlm_reward_period
    );
    assert_eq!(reward_data.eps, 0_2000000);
    assert_eq!(
        xlm_client.balance(&fee_vault),
        xlm_balance_vault_0 + xlm_rewards_0
    );
    assert_eq!(
        xlm_client.balance(&gandalf),
        xlm_balance_gandalf_0 - xlm_rewards_0
    );

    // -> skip half the reward period
    e.jump_time(xlm_reward_period / 2);

    // -> claim the rewards over this period (10k tokens issued)
    let frodo_xlm_balance_2 = xlm_client.balance(&frodo);
    let samwise_xlm_balance_2 = xlm_client.balance(&samwise);
    let merry_xlm_balance_2 = xlm_client.balance(&merry);

    let frodo_claimed_3 = fee_vault_client.claim_rewards(&frodo, &xlm, &frodo);
    let samwise_claimed_3 = fee_vault_client.claim_rewards(&samwise, &xlm, &merry);

    let frodo_xlm_balance_3 = xlm_client.balance(&frodo);
    let samwise_xlm_balance_3 = xlm_client.balance(&samwise);
    let merry_xlm_balance_3 = xlm_client.balance(&merry);
    assert_approx_eq_abs(frodo_claimed_3, 7500_0000000, 0_0001000);
    assert_approx_eq_abs(samwise_claimed_3, 2500_0000000, 0_0001000);
    assert_eq!(frodo_xlm_balance_3, frodo_xlm_balance_2 + frodo_claimed_3);
    assert_eq!(samwise_xlm_balance_3, samwise_xlm_balance_2);
    assert_eq!(merry_xlm_balance_3, merry_xlm_balance_2 + samwise_claimed_3);
    assert_eq!(
        xlm_client.balance(&fee_vault),
        xlm_balance_vault_0 + xlm_rewards_0 - (frodo_claimed_3 + samwise_claimed_3)
    );

    // -> add rewards and extend ongoing reward
    let pre_boost_xlm_rewards = fee_vault_client.get_reward_data(&xlm).unwrap();
    let xlm_rewards_1: i128 = 5_000_0000000;
    xlm_client.mint(&gandalf, &xlm_rewards_1);

    let xlm_balance_vault_2 = xlm_client.balance(&fee_vault);
    let xlm_balance_gandalf_2 = xlm_client.balance(&gandalf);
    fee_vault_client.set_rewards(
        &xlm,
        &xlm_rewards_1,
        &(e.ledger().timestamp() + xlm_reward_period),
    );
    let reward_token_result = fee_vault_client.get_reward_token().unwrap();
    assert_eq!(reward_token_result, xlm);
    let reward_data = fee_vault_client.get_reward_data(&xlm).unwrap();
    assert_eq!(reward_data.index, pre_boost_xlm_rewards.index);
    assert_eq!(reward_data.last_time, e.ledger().timestamp());
    assert_eq!(
        reward_data.expiration,
        e.ledger().timestamp() + xlm_reward_period
    );
    assert_eq!(reward_data.eps, 0_1500000);
    assert_eq!(
        xlm_client.balance(&fee_vault),
        xlm_balance_vault_2 + xlm_rewards_1
    );
    assert_eq!(
        xlm_client.balance(&gandalf),
        xlm_balance_gandalf_2 - xlm_rewards_1
    );

    // -> skip half the reward period
    e.jump_time(xlm_reward_period / 2);

    // -> have samwise withdraw their position
    let samwise_withdraw = fee_vault_client.get_underlying_tokens(&samwise);
    fee_vault_client.withdraw(&samwise, &samwise_withdraw);

    // -> skip the rest of the reward period
    e.jump_time(xlm_reward_period / 2);

    // -> claim the rewards over this period (15k tokens issued total)
    // -> frodo should earn 3/4 of the first half, and all of the second half
    // -> samwise should earn 1/4 of the first half, and none of the second half
    let frodo_xlm_balance_4 = xlm_client.balance(&frodo);
    let samwise_xlm_balance_4 = xlm_client.balance(&samwise);
    let merry_xlm_balance_4 = xlm_client.balance(&merry);

    let frodo_claimed_4 = fee_vault_client.claim_rewards(&frodo, &xlm, &frodo);
    let samwise_claimed_4 = fee_vault_client.claim_rewards(&samwise, &xlm, &merry);

    let frodo_xlm_balance_5 = xlm_client.balance(&frodo);
    let samwise_xlm_balance_5 = xlm_client.balance(&samwise);
    let merry_xlm_balance_5 = xlm_client.balance(&merry);
    assert_approx_eq_abs(frodo_claimed_4, 13125_0000000, 0_0001000);
    assert_approx_eq_abs(samwise_claimed_4, 1875_0000000, 0_0001000);
    assert_eq!(frodo_xlm_balance_5, frodo_xlm_balance_4 + frodo_claimed_4);
    assert_eq!(samwise_xlm_balance_5, samwise_xlm_balance_4);
    assert_eq!(merry_xlm_balance_5, merry_xlm_balance_4 + samwise_claimed_4);
    assert_eq!(
        xlm_client.balance(&fee_vault),
        xlm_balance_vault_2 + xlm_rewards_1 - (frodo_claimed_4 + samwise_claimed_4)
    );
}
