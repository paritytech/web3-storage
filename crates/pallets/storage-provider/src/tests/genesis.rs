// SPDX-License-Identifier: Apache-2.0

//! Tests for genesis-registered providers.

use super::*;

/// A genesis provider with non-default settings, so tests can tell the
/// settings were actually applied (registration via the extrinsic always
/// starts from `ProviderSettings::default()`).
fn genesis_provider(account: u64, stake: u64) -> GenesisProvider<Test> {
    GenesisProvider {
        account,
        multiaddr: b"/ip4/127.0.0.1/tcp/3333".to_vec(),
        public_key: vec![1u8; 32],
        stake,
        settings: ProviderSettings::<Test> {
            min_duration: 5,
            max_duration: 50,
            price_per_byte: 1,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 100,
        },
    }
}

#[test]
fn genesis_provider_is_registered_with_settings_and_reserved_stake() {
    new_test_ext_with_genesis_providers(vec![(1, 10_000)], vec![genesis_provider(1, 200)])
        .execute_with(|| {
            let provider = Providers::<Test>::get(1).expect("provider registered at genesis");
            assert_eq!(provider.multiaddr.to_vec(), b"/ip4/127.0.0.1/tcp/3333");
            assert_eq!(provider.public_key.to_vec(), vec![1u8; 32]);
            assert_eq!(provider.stake, 200);
            assert_eq!(provider.committed_bytes, 0);
            assert_eq!(provider.stats.registered_at, 0);
            assert!(provider.deregister_at.is_none());

            assert_eq!(provider.settings.min_duration, 5);
            assert_eq!(provider.settings.max_duration, 50);
            assert_eq!(provider.settings.price_per_byte, 1);
            assert!(provider.settings.accepting_primary);
            assert_eq!(provider.settings.replica_sync_price, None);
            assert!(provider.settings.accepting_extensions);
            assert_eq!(provider.settings.max_capacity, 100);

            assert_eq!(Balances::reserved_balance(1), 200);
            assert_eq!(Balances::free_balance(1), 9_800);
        });
}

#[test]
fn register_provider_after_genesis_fails_with_already_registered() {
    // The PreviewNet wipe scenario: the default provider comes back
    // pre-registered, so a manual re-registration must fail.
    new_test_ext_with_genesis_providers(vec![(1, 10_000)], vec![genesis_provider(1, 200)])
        .execute_with(|| {
            assert_noop!(
                StorageProvider::register_provider(
                    RuntimeOrigin::signed(1),
                    b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap(),
                    test_public_key(),
                    200
                ),
                Error::<Test>::ProviderAlreadyRegistered
            );
        });
}

#[test]
#[should_panic(expected = "genesis provider registration should not fail")]
fn genesis_provider_below_min_stake_panics() {
    // MinProviderStake is 100 in the mock.
    let _ = new_test_ext_with_genesis_providers(vec![(1, 10_000)], vec![genesis_provider(1, 50)]);
}

#[test]
#[should_panic(expected = "genesis provider registration should not fail")]
fn genesis_provider_unfunded_account_panics() {
    let _ = new_test_ext_with_genesis_providers(vec![], vec![genesis_provider(1, 200)]);
}

#[test]
#[should_panic(expected = "genesis provider registration should not fail")]
fn genesis_provider_invalid_durations_panic() {
    let mut provider = genesis_provider(1, 200);
    provider.settings.min_duration = 100;
    provider.settings.max_duration = 10;
    let _ = new_test_ext_with_genesis_providers(vec![(1, 10_000)], vec![provider]);
}

#[test]
#[should_panic(expected = "genesis provider registration should not fail")]
fn genesis_provider_capacity_unbacked_by_stake_panics() {
    // MinStakePerByte is 1 in the mock: capacity 1000 needs stake >= 1000.
    let mut provider = genesis_provider(1, 200);
    provider.settings.max_capacity = 1_000;
    let _ = new_test_ext_with_genesis_providers(vec![(1, 10_000)], vec![provider]);
}

#[test]
fn genesis_config_json_roundtrip() {
    // Pins the JSON shape embedded in chain specs by chain-spec-builder:
    // camelCase keys and 0x-hex byte fields.
    let config = crate::pallet::GenesisConfig::<Test> {
        buckets: vec![],
        providers: vec![genesis_provider(1, 200)],
    };

    let json = serde_json::to_value(&config).unwrap();
    let provider = &json["providers"][0];
    assert_eq!(
        provider["multiaddr"],
        "0x2f6970342f3132372e302e302e312f7463702f33333333"
    );
    assert_eq!(provider["publicKey"], format!("0x{}", "01".repeat(32)));
    assert_eq!(provider["settings"]["minDuration"], 5);
    assert_eq!(provider["settings"]["maxCapacity"], 100);

    let decoded: crate::pallet::GenesisConfig<Test> = serde_json::from_value(json).unwrap();
    assert_eq!(decoded.providers, config.providers);
}
