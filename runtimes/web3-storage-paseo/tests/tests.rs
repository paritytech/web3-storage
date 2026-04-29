#![cfg(test)]

use frame_support::{assert_ok, dispatch::GetDispatchInfo, traits::Currency};
use pallet_storage_provider::Call as StorageProviderCall;
use parachains_common::{AccountId, AuraId, Hash as PcHash, Signature as PcSignature};
use parachains_runtimes_test_utils::{ExtBuilder, RuntimeHelper};
use sp_core::{crypto::Ss58Codec, Encode, Pair};
use sp_keyring::Sr25519Keyring;
use sp_runtime::{transaction_validity, ApplyExtrinsicResult, BuildStorage};
use storage_paseo_runtime::{
    paseo_constants::currency::UNIT, xcm_config::LocationToAccountId, AllPalletsWithoutSystem,
    Balance, Balances, Block, Runtime, RuntimeCall, RuntimeEvent, RuntimeGenesisConfig,
    RuntimeOrigin, SessionKeys, StorageProvider, System, TxExtension, UncheckedExtrinsic,
    WeightToFee,
};
use xcm::latest::prelude::*;
use xcm_runtime_apis::conversions::LocationToAccountHelper;

fn construct_extrinsic(
    sender: Option<sp_core::sr25519::Pair>,
    call: RuntimeCall,
) -> Result<UncheckedExtrinsic, transaction_validity::TransactionValidityError> {
    // provide a known block hash for the immortal era check
    frame_system::BlockHash::<Runtime>::insert(0, PcHash::default());
    let inner = (
        frame_system::CheckNonZeroSender::<Runtime>::new(),
        frame_system::CheckSpecVersion::<Runtime>::new(),
        frame_system::CheckTxVersion::<Runtime>::new(),
        frame_system::CheckGenesis::<Runtime>::new(),
        frame_system::CheckEra::<Runtime>::from(sp_runtime::generic::Era::immortal()),
        frame_system::CheckNonce::<Runtime>::from(if let Some(s) = sender.as_ref() {
            let account_id = AccountId::from(s.public());
            frame_system::Pallet::<Runtime>::account(&account_id).nonce
        } else {
            0
        }),
        frame_system::CheckWeight::<Runtime>::new(),
        pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(0u128),
    );
    let tx_ext: TxExtension =
        cumulus_pallet_weight_reclaim::StorageWeightReclaim::<Runtime, _>::from(inner);

    if let Some(s) = sender.as_ref() {
        // Signed call.
        let account_id = AccountId::from(s.public());
        let payload = sp_runtime::generic::SignedPayload::new(call.clone(), tx_ext.clone())?;
        let signature = payload.using_encoded(|e| s.sign(e));
        Ok(UncheckedExtrinsic::new_signed(
            call,
            account_id.into(),
            PcSignature::Sr25519(signature),
            tx_ext,
        ))
    } else {
        // Unsigned call.
        Ok(UncheckedExtrinsic::new_transaction(call, tx_ext))
    }
}

fn construct_and_apply_extrinsic(
    account: Option<sp_core::sr25519::Pair>,
    call: RuntimeCall,
) -> ApplyExtrinsicResult {
    let dispatch_info = call.get_dispatch_info();
    let xt = construct_extrinsic(account, call)?;
    let xt_len = xt.encode().len();
    log::info!(
        "Applying extrinsic: class={:?} pays_fee={:?} weight={:?} encoded_len={} bytes",
        dispatch_info.class,
        dispatch_info.pays_fee,
        dispatch_info.total_weight(),
        xt_len
    );
    storage_paseo_runtime::Executive::apply_extrinsic(xt)
}

fn assert_ok_ok(apply_result: ApplyExtrinsicResult) {
    assert_ok!(apply_result);
    assert_ok!(apply_result.unwrap());
}

/// Build a 32-byte raw public key for the given test keyring as a BoundedVec.
fn to_provider_public_key(
    who: Sr25519Keyring,
) -> frame_support::BoundedVec<u8, frame_support::traits::ConstU32<64>> {
    who.to_raw_public().to_vec().try_into().unwrap()
}

/// Default stake used in provider tests: 10x the runtime minimum.
fn default_stake() -> Balance {
    <Runtime as pallet_storage_provider::Config>::MinProviderStake::get().saturating_mul(10)
}

/// Register `account` as a provider with `stake`, funding the account first.
///
/// Used by tests that need an already-registered provider as a precondition.
fn register_provider_for(account: Sr25519Keyring, stake: Balance) {
    let who: AccountId = account.to_account_id();
    let _ = Balances::deposit_creating(&who, stake.saturating_mul(2));

    assert_ok_ok(construct_and_apply_extrinsic(
        Some(account.pair()),
        RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::register_provider {
            multiaddr: b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap(),
            public_key: to_provider_public_key(account),
            stake,
        }),
    ));
}

fn new_test_ext() -> sp_io::TestExternalities {
    sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
}

#[test]
fn should_register_provider() {
    new_test_ext().execute_with(|| {
        let account = Sr25519Keyring::Alice;
        let who: AccountId = account.to_account_id();
        let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
        let public_key = to_provider_public_key(account);
        let stake_amount = default_stake();

        // Fund the account so it can reserve the stake.
        let _ = Balances::deposit_creating(&who, stake_amount.saturating_mul(2));

        assert_ok_ok(construct_and_apply_extrinsic(
            Some(account.pair()),
            RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::register_provider {
                multiaddr: multiaddr.clone().try_into().unwrap(),
                public_key: public_key.clone(),
                stake: stake_amount,
            }),
        ));

        let provider = StorageProvider::providers(&who).expect("provider must be stored");
        assert_eq!(provider.stake, stake_amount);
        assert_eq!(provider.multiaddr.to_vec(), multiaddr);
        assert_eq!(provider.public_key, public_key);
    });
}

#[test]
fn should_fail_register_provider_with_insufficient_stake() {
    new_test_ext().execute_with(|| {
        let account = Sr25519Keyring::Alice;
        let who: AccountId = account.to_account_id();
        let stake_amount: Balance =
            <Runtime as pallet_storage_provider::Config>::MinProviderStake::get() - 1;

        let _ = Balances::deposit_creating(&who, default_stake().saturating_mul(2));

        let result = construct_and_apply_extrinsic(
            Some(account.pair()),
            RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::register_provider {
                multiaddr: b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap(),
                public_key: to_provider_public_key(account),
                stake: stake_amount,
            }),
        );

        // Extrinsic applies but the dispatch returns an error.
        let dispatch_outcome = result.expect("extrinsic should be applied");
        assert!(
            dispatch_outcome.is_err(),
            "expected InsufficientStake dispatch error"
        );
        assert!(StorageProvider::providers(&who).is_none());
    });
}

#[test]
fn should_add_stake_to_existing_provider() {
    new_test_ext().execute_with(|| {
        let account = Sr25519Keyring::Alice;
        let who: AccountId = account.to_account_id();
        let initial_stake = default_stake();
        register_provider_for(account, initial_stake);

        let extra = 500 * UNIT;
        // Top up balance so the additional reserve succeeds.
        let _ = Balances::deposit_creating(&who, extra.saturating_mul(2));

        assert_ok_ok(construct_and_apply_extrinsic(
            Some(account.pair()),
            RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::add_stake {
                amount: extra,
            }),
        ));

        let provider = StorageProvider::providers(&who).expect("provider must be stored");
        assert_eq!(provider.stake, initial_stake + extra);
    });
}

#[test]
fn should_deregister_provider_and_return_stake() {
    new_test_ext().execute_with(|| {
        let account = Sr25519Keyring::Alice;
        let who: AccountId = account.to_account_id();
        let stake = default_stake();
        register_provider_for(account, stake);

        let reserved_before = Balances::reserved_balance(&who);
        assert_eq!(reserved_before, stake);

        assert_ok_ok(construct_and_apply_extrinsic(
            Some(account.pair()),
            RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::deregister_provider {}),
        ));

        assert!(StorageProvider::providers(&who).is_none());
        assert_eq!(Balances::reserved_balance(&who), 0);
    });
}

#[test]
fn should_update_provider_settings() {
    new_test_ext().execute_with(|| {
        let account = Sr25519Keyring::Alice;
        let who: AccountId = account.to_account_id();
        register_provider_for(account, default_stake());

        let new_settings = pallet_storage_provider::ProviderSettings::<Runtime> {
            min_duration: 10,
            max_duration: 1_000,
            price_per_byte: 7,
            accepting_primary: false,
            replica_sync_price: Some(3),
            accepting_extensions: false,
            max_capacity: 1_024 * 1_024,
        };

        assert_ok_ok(construct_and_apply_extrinsic(
            Some(account.pair()),
            RuntimeCall::StorageProvider(
                StorageProviderCall::<Runtime>::update_provider_settings {
                    settings: new_settings.clone(),
                },
            ),
        ));

        let provider = StorageProvider::providers(&who).expect("provider must be stored");
        assert_eq!(provider.settings, new_settings);
    });
}

#[test]
fn should_create_bucket() {
    new_test_ext().execute_with(|| {
        let account = Sr25519Keyring::Alice;
        let who: AccountId = account.to_account_id();
        // Fund the caller so any tx fees can be paid.
        let _ = Balances::deposit_creating(&who, default_stake());

        let bucket_id_before = pallet_storage_provider::NextBucketId::<Runtime>::get();

        assert_ok_ok(construct_and_apply_extrinsic(
            Some(account.pair()),
            RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::create_bucket {
                min_providers: 1,
            }),
        ));

        let bucket = pallet_storage_provider::Buckets::<Runtime>::get(bucket_id_before)
            .expect("bucket must be stored");
        assert_eq!(bucket.min_providers, 1);
        assert_eq!(
            pallet_storage_provider::NextBucketId::<Runtime>::get(),
            bucket_id_before + 1
        );

        let owned = pallet_storage_provider::MemberBuckets::<Runtime>::get(&who);
        assert!(owned.contains(&bucket_id_before));
    });
}

const ALICE: [u8; 32] = [1u8; 32];

#[test]
fn location_conversion_works() {
    // the purpose of hardcoded values is to catch an unintended location conversion logic change.
    struct TestCase {
        description: &'static str,
        location: Location,
        expected_account_id_str: &'static str,
    }

    let test_cases = vec![
        // DescribeTerminus
        TestCase {
            description: "DescribeTerminus Parent",
            location: Location::new(1, Here),
            expected_account_id_str: "5Dt6dpkWPwLaH4BBCKJwjiWrFVAGyYk3tLUabvyn4v7KtESG",
        },
        TestCase {
            description: "DescribeTerminus Sibling",
            location: Location::new(1, [Parachain(1111)]),
            expected_account_id_str: "5Eg2fnssmmJnF3z1iZ1NouAuzciDaaDQH7qURAy3w15jULDk",
        },
        // DescribePalletTerminal
        TestCase {
            description: "DescribePalletTerminal Parent",
            location: Location::new(1, [PalletInstance(50)]),
            expected_account_id_str: "5CnwemvaAXkWFVwibiCvf2EjqwiqBi29S5cLLydZLEaEw6jZ",
        },
        TestCase {
            description: "DescribePalletTerminal Sibling",
            location: Location::new(1, [Parachain(1111), PalletInstance(50)]),
            expected_account_id_str: "5GFBgPjpEQPdaxEnFirUoa51u5erVx84twYxJVuBRAT2UP2g",
        },
        // DescribeAccountId32Terminal
        TestCase {
            description: "DescribeAccountId32Terminal Parent",
            location: Location::new(
                1,
                [Junction::AccountId32 {
                    network: None,
                    id: AccountId::from(ALICE).into(),
                }],
            ),
            expected_account_id_str: "5DN5SGsuUG7PAqFL47J9meViwdnk9AdeSWKFkcHC45hEzVz4",
        },
        TestCase {
            description: "DescribeAccountId32Terminal Sibling",
            location: Location::new(
                1,
                [
                    Parachain(1111),
                    Junction::AccountId32 {
                        network: None,
                        id: AccountId::from(ALICE).into(),
                    },
                ],
            ),
            expected_account_id_str: "5DGRXLYwWGce7wvm14vX1Ms4Vf118FSWQbJkyQigY2pfm6bg",
        },
        // DescribeAccountKey20Terminal
        TestCase {
            description: "DescribeAccountKey20Terminal Parent",
            location: Location::new(
                1,
                [AccountKey20 {
                    network: None,
                    key: [0u8; 20],
                }],
            ),
            expected_account_id_str: "5F5Ec11567pa919wJkX6VHtv2ZXS5W698YCW35EdEbrg14cg",
        },
        TestCase {
            description: "DescribeAccountKey20Terminal Sibling",
            location: Location::new(
                1,
                [
                    Parachain(1111),
                    AccountKey20 {
                        network: None,
                        key: [0u8; 20],
                    },
                ],
            ),
            expected_account_id_str: "5CB2FbUds2qvcJNhDiTbRZwiS3trAy6ydFGMSVutmYijpPAg",
        },
        // DescribeTreasuryVoiceTerminal
        TestCase {
            description: "DescribeTreasuryVoiceTerminal Parent",
            location: Location::new(
                1,
                [Plurality {
                    id: BodyId::Treasury,
                    part: BodyPart::Voice,
                }],
            ),
            expected_account_id_str: "5CUjnE2vgcUCuhxPwFoQ5r7p1DkhujgvMNDHaF2bLqRp4D5F",
        },
        TestCase {
            description: "DescribeTreasuryVoiceTerminal Sibling",
            location: Location::new(
                1,
                [
                    Parachain(1111),
                    Plurality {
                        id: BodyId::Treasury,
                        part: BodyPart::Voice,
                    },
                ],
            ),
            expected_account_id_str: "5G6TDwaVgbWmhqRUKjBhRRnH4ry9L9cjRymUEmiRsLbSE4gB",
        },
        // DescribeBodyTerminal
        TestCase {
            description: "DescribeBodyTerminal Parent",
            location: Location::new(
                1,
                [Plurality {
                    id: BodyId::Unit,
                    part: BodyPart::Voice,
                }],
            ),
            expected_account_id_str: "5EBRMTBkDisEXsaN283SRbzx9Xf2PXwUxxFCJohSGo4jYe6B",
        },
        TestCase {
            description: "DescribeBodyTerminal Sibling",
            location: Location::new(
                1,
                [
                    Parachain(1111),
                    Plurality {
                        id: BodyId::Unit,
                        part: BodyPart::Voice,
                    },
                ],
            ),
            expected_account_id_str: "5DBoExvojy8tYnHgLL97phNH975CyT45PWTZEeGoBZfAyRMH",
        },
    ];

    for tc in test_cases {
        let expected =
            AccountId::from_string(tc.expected_account_id_str).expect("Invalid AccountId string");

        let got = LocationToAccountHelper::<AccountId, LocationToAccountId>::convert_location(
            tc.location.into(),
        )
        .unwrap();

        assert_eq!(got, expected, "{}", tc.description);
    }
}

#[test]
fn xcm_payment_api_works() {
    parachains_runtimes_test_utils::test_cases::xcm_payment_api_with_native_token_works::<
        Runtime,
        RuntimeCall,
        RuntimeOrigin,
        Block,
        WeightToFee,
    >();
}

#[test]
fn should_register_provider_via_xcm() {
    let account = Sr25519Keyring::Alice;
    let who: AccountId = account.to_account_id();
    let stake = default_stake();
    let multiaddr: Vec<u8> = b"/ip4/127.0.0.1/tcp/3000".to_vec();
    let public_key = to_provider_public_key(account);

    let register_provider_call =
        RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::register_provider {
            multiaddr: multiaddr.clone().try_into().unwrap(),
            public_key: public_key.clone(),
            stake,
        });

    ExtBuilder::<Runtime>::default()
        .with_collators(vec![AccountId::from(ALICE)])
        .with_session_keys(vec![(
            AccountId::from(ALICE),
            AccountId::from(ALICE),
            SessionKeys {
                aura: AuraId::from(sp_core::sr25519::Public::from_raw(ALICE)),
            },
        )])
        .with_tracing()
        .build()
        .execute_with(|| {
            // Alice needs balance for both: paid XCM execution fees and the stake reserve.
            let _ = Balances::deposit_creating(&who, stake.saturating_mul(4));

            // Alice's local account location. With `OriginKind::SovereignAccount`,
            // `AccountId32Aliases` (inside `LocationToAccountId`) maps this directly to
            // Alice's `AccountId` and `SovereignSignedViaLocation` produces `Signed(Alice)` —
            // so `register_provider` registers Alice herself.
            let alice_location = Location::new(
                0,
                [Junction::AccountId32 {
                    network: None,
                    id: account.to_raw_public(),
                }],
            );

            // Pay execution fees from Alice's balance — her location isn't in the
            // unpaid-execution allowlist, so we have to clear `AllowTopLevelPaidExecutionFrom`.
            let fee: Asset = (Location::parent(), UNIT).into();

            assert_ok!(
                RuntimeHelper::<Runtime, AllPalletsWithoutSystem>::execute_as_origin(
                    (alice_location, OriginKind::SovereignAccount),
                    register_provider_call,
                    Some(fee),
                )
                .ensure_complete()
            );

            System::assert_has_event(RuntimeEvent::StorageProvider(
                pallet_storage_provider::Event::ProviderRegistered {
                    provider: who.clone(),
                    stake,
                },
            ));

            let provider = StorageProvider::providers(&who).expect("Alice must be registered");
            assert_eq!(provider.stake, stake);
            assert_eq!(provider.multiaddr.to_vec(), multiaddr);
            assert_eq!(provider.public_key, public_key);
        });
}

/// Alice's account on a sibling parachain (`(1, [Parachain(para_id), AccountId32 { Alice }])`).
///
/// `LocationToAccountId` converts this through `HashedDescription` to a derived sovereign
/// `AccountId` — that derived account becomes the `Signed` origin of the dispatched call.
fn alice_on_sibling_parachain(para_id: u32) -> Location {
    Location::new(
        1,
        [
            Parachain(para_id),
            Junction::AccountId32 {
                network: None,
                id: Sr25519Keyring::Alice.to_raw_public(),
            },
        ],
    )
}

fn xcm_test_ext() -> sp_io::TestExternalities {
    ExtBuilder::<Runtime>::default()
        .with_collators(vec![AccountId::from(ALICE)])
        .with_session_keys(vec![(
            AccountId::from(ALICE),
            AccountId::from(ALICE),
            SessionKeys {
                aura: AuraId::from(sp_core::sr25519::Public::from_raw(ALICE)),
            },
        )])
        .with_tracing()
        .build()
}

#[test]
fn should_add_stake_via_xcm() {
    let alice_on_para = alice_on_sibling_parachain(2_000);
    let initial_stake = default_stake();
    let extra = 500 * UNIT;

    let register_call =
        RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::register_provider {
            multiaddr: b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap(),
            public_key: to_provider_public_key(Sr25519Keyring::Alice),
            stake: initial_stake,
        });
    let add_stake_call =
        RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::add_stake { amount: extra });

    xcm_test_ext().execute_with(|| {
        // The dispatch origin is the sovereign `AccountId` derived from Alice-on-para.
        let derived: AccountId =
            LocationToAccountHelper::<AccountId, LocationToAccountId>::convert_location(
                alice_on_para.clone().into(),
            )
            .expect("Alice-on-para must convert to an account");

        // Cover stake reserves (initial + extra) and XCM execution fees with margin.
        let _ = Balances::deposit_creating(
            &derived,
            initial_stake.saturating_add(extra).saturating_mul(4),
        );
        let fee: Asset = (Location::parent(), UNIT).into();

        // 1. Register Alice-on-para as a provider.
        assert_ok!(
            RuntimeHelper::<Runtime, AllPalletsWithoutSystem>::execute_as_origin(
                (alice_on_para.clone(), OriginKind::SovereignAccount),
                register_call,
                Some(fee.clone()),
            )
            .ensure_complete()
        );

        // 2. Top up the stake from the same origin.
        assert_ok!(
            RuntimeHelper::<Runtime, AllPalletsWithoutSystem>::execute_as_origin(
                (alice_on_para, OriginKind::SovereignAccount),
                add_stake_call,
                Some(fee),
            )
            .ensure_complete()
        );

        let provider = StorageProvider::providers(&derived).expect("provider must be stored");
        assert_eq!(provider.stake, initial_stake + extra);
    });
}

#[test]
fn should_register_provider_via_xcm_from_sibling_parachain() {
    // Use a different para id from `should_add_stake_via_xcm` to make the derived sovereign
    // distinct, even though each test runs in a fresh ext.
    let alice_on_para = alice_on_sibling_parachain(3_000);
    let stake = default_stake();

    let register_call =
        RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::register_provider {
            multiaddr: b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap(),
            public_key: to_provider_public_key(Sr25519Keyring::Alice),
            stake,
        });

    xcm_test_ext().execute_with(|| {
        let derived: AccountId =
            LocationToAccountHelper::<AccountId, LocationToAccountId>::convert_location(
                alice_on_para.clone().into(),
            )
            .expect("Alice-on-para must convert to an account");

        let _ = Balances::deposit_creating(&derived, stake.saturating_mul(4));
        let fee: Asset = (Location::parent(), UNIT).into();

        assert_ok!(
            RuntimeHelper::<Runtime, AllPalletsWithoutSystem>::execute_as_origin(
                (alice_on_para, OriginKind::SovereignAccount),
                register_call,
                Some(fee),
            )
            .ensure_complete()
        );

        let provider = StorageProvider::providers(&derived).expect("provider must be stored");
        assert_eq!(provider.stake, stake);
    });
}

#[test]
fn should_fail_xcm_unpaid_execution_from_unauthorized_origin() {
    // Alice's account on the relay chain: `(1, [AccountId32 { Alice }])`.
    // This matches none of the unpaid-execution allowlists
    // (`ParentOrParentsExecutivePlurality`, `FellowsPlurality`, `Equals<GovernanceLocation>`,
    // `IsSiblingParachain`), so the barrier must reject an unpaid message from here.
    let alice_on_relay = Location::new(
        1,
        [Junction::AccountId32 {
            network: None,
            id: Sr25519Keyring::Alice.to_raw_public(),
        }],
    );

    let register_call =
        RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::register_provider {
            multiaddr: b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap(),
            public_key: to_provider_public_key(Sr25519Keyring::Alice),
            stake: default_stake(),
        });

    xcm_test_ext().execute_with(|| {
        // `None` fee → `RuntimeHelper` builds an `UnpaidExecution + Transact` message.
        let outcome = RuntimeHelper::<Runtime, AllPalletsWithoutSystem>::execute_as_origin(
            (alice_on_relay, OriginKind::SovereignAccount),
            register_call,
            None,
        );
        assert!(
            outcome.clone().ensure_complete().is_err(),
            "barrier must reject unauthorized unpaid execution, got: {outcome:?}",
        );
    });
}
