#![cfg(test)]

//! Tests for the Susu Factory contract.
//!
//! These cover configuration, access control, validation and the pause switch.
//! The deploy-a-group path needs the compiled Group Wasm and is covered by the
//! feature-gated integration test in `tests/deploy_group.rs`, which CI runs after
//! building the Wasm.

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Events as _},
    Event,
};

/// Registers a Factory whose group Wasm hash is a placeholder.
///
/// The hash is only consumed by `create_group`, which is not exercised here, so a
/// placeholder is sufficient and keeps these tests independent of the build order.
fn setup(fee_bps: u32) -> (Env, Address, Address, FactoryContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let wasm_hash = BytesN::from_array(&env, &[7u8; 32]);
    let factory_id = env.register(
        FactoryContract,
        (admin.clone(), wasm_hash, treasury.clone(), fee_bps),
    );
    let client = FactoryContractClient::new(&env, &factory_id);
    (env, admin, treasury, client)
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

#[test]
fn constructor_stores_configuration() {
    let (_, admin, treasury, client) = setup(MAX_FEE_BPS);

    let config = client.get_config();
    assert_eq!(config.admin, admin);
    assert_eq!(config.treasury, treasury);
    assert_eq!(config.fee_bps, MAX_FEE_BPS);
    assert!(!config.paused, "a new Factory is not paused");
    assert_eq!(client.get_group_count(), 0);
    assert_eq!(client.version(), 2);
}

#[test]
#[should_panic]
fn constructor_rejects_fee_above_the_protocol_maximum() {
    setup(MAX_FEE_BPS + 1);
}

#[test]
#[should_panic]
fn constructor_rejects_zero_fee() {
    setup(0);
}

// ---------------------------------------------------------------------------
// Fee configuration
// ---------------------------------------------------------------------------

#[test]
fn set_fee_updates_future_groups_within_the_cap() {
    let (env, _, _, client) = setup(MAX_FEE_BPS);

    client.set_fee(&10u32);
    // `events().all()` reflects the most recent invocation, so capture the
    // emitting call's events before making any further contract call.
    let emitted = env.events().all().filter_by_contract(&client.address);

    assert_eq!(client.get_config().fee_bps, 10);
    let expected = FeeUpdated { fee_bps: 10 }.to_xdr(&env, &client.address);
    assert!(emitted.events().contains(&expected));
}

#[test]
fn set_fee_accepts_the_cap_boundary() {
    let (_, _, _, client) = setup(1);
    client.set_fee(&MAX_FEE_BPS);
    assert_eq!(client.get_config().fee_bps, MAX_FEE_BPS);
}

#[test]
fn set_fee_rejects_a_fee_above_the_cap() {
    let (_, _, _, client) = setup(MAX_FEE_BPS);

    assert_eq!(
        client.try_set_fee(&(MAX_FEE_BPS + 1)),
        Err(Ok(FactoryError::InvalidFeeBps)),
        "the protocol can never charge more than MAX_FEE_BPS"
    );
    assert_eq!(
        client.try_set_fee(&u32::MAX),
        Err(Ok(FactoryError::InvalidFeeBps))
    );
    assert_eq!(
        client.try_set_fee(&0u32),
        Err(Ok(FactoryError::InvalidFeeBps))
    );
    // Unchanged by the failed attempts.
    assert_eq!(client.get_config().fee_bps, MAX_FEE_BPS);
}

#[test]
fn set_fee_requires_admin_authorization() {
    let (env, _, _, client) = setup(MAX_FEE_BPS);
    env.set_auths(&[]);

    assert!(client.try_set_fee(&10u32).is_err());
}

// ---------------------------------------------------------------------------
// Treasury configuration
// ---------------------------------------------------------------------------

#[test]
fn set_treasury_updates_future_groups() {
    let (env, _, _, client) = setup(MAX_FEE_BPS);
    let new_treasury = Address::generate(&env);

    client.set_treasury(&new_treasury);
    let emitted = env.events().all().filter_by_contract(&client.address);

    assert_eq!(client.get_config().treasury, new_treasury);
    let expected = TreasuryUpdated {
        treasury: new_treasury,
    }
    .to_xdr(&env, &client.address);
    assert!(emitted.events().contains(&expected));
}

#[test]
fn set_treasury_requires_admin_authorization() {
    let (env, _, _, client) = setup(MAX_FEE_BPS);
    env.set_auths(&[]);
    let new_treasury = Address::generate(&env);

    assert!(client.try_set_treasury(&new_treasury).is_err());
}

// ---------------------------------------------------------------------------
// Pause
// ---------------------------------------------------------------------------

#[test]
fn pause_and_unpause_toggle_new_group_creation() {
    let (env, _, _, client) = setup(MAX_FEE_BPS);
    assert!(!client.get_config().paused);

    client.pause();
    let paused_events = env.events().all().filter_by_contract(&client.address);
    assert!(client.get_config().paused);

    client.unpause();
    let unpaused_events = env.events().all().filter_by_contract(&client.address);
    assert!(!client.get_config().paused);

    assert!(paused_events
        .events()
        .contains(&PauseUpdated { paused: true }.to_xdr(&env, &client.address)));
    assert!(unpaused_events
        .events()
        .contains(&PauseUpdated { paused: false }.to_xdr(&env, &client.address)));
}

#[test]
fn pause_requires_admin_authorization() {
    let (env, _, _, client) = setup(MAX_FEE_BPS);
    env.set_auths(&[]);

    assert!(client.try_pause().is_err());
    assert!(client.try_unpause().is_err());
}

// ---------------------------------------------------------------------------
// Group lookup
// ---------------------------------------------------------------------------

#[test]
fn get_group_fails_for_an_unknown_id() {
    let (_, _, _, client) = setup(MAX_FEE_BPS);

    assert_eq!(
        client.try_get_group(&1u32),
        Err(Ok(FactoryError::GroupNotFound))
    );
    assert_eq!(
        client.try_get_group(&u32::MAX),
        Err(Ok(FactoryError::GroupNotFound))
    );
}

// ---------------------------------------------------------------------------
// create_group validation
//
// These run before any deployment, so they need no Group Wasm. A placeholder hash
// is fine: the call must be rejected before it is ever used.
// ---------------------------------------------------------------------------

#[test]
fn create_group_requires_the_creators_authorization() {
    let (env, _, _, client) = setup(MAX_FEE_BPS);
    env.set_auths(&[]);
    let creator = Address::generate(&env);
    let token = Address::generate(&env);

    assert!(client
        .try_create_group(&creator, &token, &(10_000_000i128), &3u32, &604_800u64)
        .is_err());
}

#[test]
fn create_group_rejects_invalid_parameters() {
    let (env, _, _, client) = setup(MAX_FEE_BPS);
    let creator = Address::generate(&env);
    let token = Address::generate(&env);

    assert_eq!(
        client.try_create_group(&creator, &token, &0i128, &3u32, &604_800u64),
        Err(Ok(FactoryError::InvalidContributionAmount))
    );
    assert_eq!(
        client.try_create_group(&creator, &token, &(-1i128), &3u32, &604_800u64),
        Err(Ok(FactoryError::InvalidContributionAmount))
    );
    assert_eq!(
        client.try_create_group(&creator, &token, &i128::MAX, &2u32, &604_800u64),
        Err(Ok(FactoryError::InvalidContributionAmount))
    );
    assert_eq!(
        client.try_create_group(&creator, &token, &(i128::MAX / 3 + 1), &3u32, &604_800u64),
        Err(Ok(FactoryError::InvalidContributionAmount))
    );
    assert_eq!(
        client.try_create_group(&creator, &token, &10_000_000i128, &0u32, &604_800u64),
        Err(Ok(FactoryError::InvalidMemberCapacity))
    );
    assert_eq!(
        client.try_create_group(&creator, &token, &10_000_000i128, &1u32, &604_800u64),
        Err(Ok(FactoryError::InvalidMemberCapacity))
    );
    assert_eq!(
        client.try_create_group(
            &creator,
            &token,
            &10_000_000i128,
            &(MAX_MEMBERS + 1),
            &604_800u64
        ),
        Err(Ok(FactoryError::InvalidMemberCapacity))
    );
    assert_eq!(
        client.try_create_group(&creator, &token, &10_000_000i128, &3u32, &0u64),
        Err(Ok(FactoryError::InvalidFrequency))
    );
}

#[test]
fn create_group_is_refused_while_paused() {
    let (env, _, _, client) = setup(MAX_FEE_BPS);
    let creator = Address::generate(&env);
    let token = Address::generate(&env);
    client.pause();

    assert_eq!(
        client.try_create_group(&creator, &token, &10_000_000i128, &3u32, &604_800u64),
        Err(Ok(FactoryError::Paused)),
        "pausing must block new groups"
    );
}

#[test]
#[should_panic]
fn constructor_rejects_treasury_equal_to_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let wasm_hash = BytesN::from_array(&env, &[7u8; 32]);
    env.register(
        FactoryContract,
        (admin.clone(), wasm_hash, admin.clone(), MAX_FEE_BPS),
    );
}

#[test]
fn set_treasury_rejects_admin_or_factory_address() {
    let (_env, admin, _, client) = setup(MAX_FEE_BPS);
    let factory_addr = client.address.clone();

    let res_admin = client.try_set_treasury(&admin);
    assert_eq!(res_admin, Err(Ok(FactoryError::InvalidTreasury)));

    let res_factory = client.try_set_treasury(&factory_addr);
    assert_eq!(res_factory, Err(Ok(FactoryError::InvalidTreasury)));
}
