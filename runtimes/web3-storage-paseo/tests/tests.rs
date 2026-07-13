// SPDX-License-Identifier: GPL-3.0-only

#![cfg(test)]

use frame_support::{
    assert_ok, dispatch::GetDispatchInfo, pallet_prelude::Hooks, traits::Currency,
};
use pallet_drive_registry::Call as DriveRegistryCall;
use pallet_s3_registry::Call as S3RegistryCall;
use pallet_storage_provider::Call as StorageProviderCall;
use parachains_common::{AccountId, AuraId, Hash as PcHash, Signature as PcSignature};
use parachains_runtimes_test_utils::{ExtBuilder, RuntimeHelper};
use sp_core::{crypto::Ss58Codec, Encode, Pair};
use sp_keyring::Sr25519Keyring;
use sp_runtime::{transaction_validity, ApplyExtrinsicResult, BuildStorage};
use storage_paseo_runtime::{
    paseo_constants::currency::UNIT, xcm_config::LocationToAccountId, AllPalletsWithoutSystem,
    Balance, Balances, Block, BlockNumber, Runtime, RuntimeCall, RuntimeEvent,
    RuntimeGenesisConfig, RuntimeOrigin, S3Registry, SessionKeys, StorageProvider, System,
    TxExtension, UncheckedExtrinsic, WeightToFee,
};
use storage_primitives::AgreementTerms;
use xcm::latest::prelude::*;
use xcm_runtime_apis::conversions::LocationToAccountHelper;

//
// Utility functions & constants
//

const ALICE: [u8; 32] = [1u8; 32];

/// Set both clocks the runtime reads: `System` (parachain height) and the
/// relay-chain number served by `RelaychainDataProvider`, which the storage
/// pallets measure all durations against. Kept in lockstep for simplicity.
fn set_clocks(n: BlockNumber) {
    use sp_runtime::traits::BlockNumberProvider;
    System::set_block_number(n);
    cumulus_pallet_parachain_system::RelaychainDataProvider::<Runtime>::set_block_number(n);
}

/// Advance to the next block for testing transaction storage.
fn advance_block() {
    let current = frame_system::Pallet::<Runtime>::block_number();

    <StorageProvider as Hooks<_>>::on_finalize(current);
    <System as Hooks<_>>::on_finalize(current);

    let next = current + 1;
    set_clocks(next);

    frame_system::BlockWeight::<Runtime>::kill();
    frame_system::BlockSize::<Runtime>::kill();

    <System as Hooks<_>>::on_initialize(next);
    <StorageProvider as Hooks<_>>::on_initialize(next);
}

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
        frame_metadata_hash_extension::CheckMetadataHash::<Runtime>::new(false),
        // SetOrigin's substrate-signed path is a no-op; only the eth path
        // (built by `EthExtraImpl::get_eth_extension`) sets `is_eth_transaction`.
        pallet_revive::evm::tx_extension::SetOrigin::<Runtime>::default(),
    );
    let tx_ext: TxExtension =
        cumulus_pallet_weight_reclaim::StorageWeightReclaim::<Runtime, _>::from(inner);

    // `pallet_revive`'s `UncheckedExtrinsic` wraps `generic::UncheckedExtrinsic` and
    // only exposes constructors through traits — go through the wrapped form so the
    // test helper stays readable.
    let inner_xt = if let Some(s) = sender.as_ref() {
        let account_id = AccountId::from(s.public());
        let payload = sp_runtime::generic::SignedPayload::new(call.clone(), tx_ext.clone())?;
        let signature = payload.using_encoded(|e| s.sign(e));
        sp_runtime::generic::UncheckedExtrinsic::new_signed(
            call,
            account_id.into(),
            PcSignature::Sr25519(signature),
            tx_ext,
        )
    } else {
        sp_runtime::generic::UncheckedExtrinsic::new_transaction(call, tx_ext)
    };
    Ok(UncheckedExtrinsic::from(inner_xt))
}

fn construct_and_apply_extrinsic(
    account: Option<sp_core::sr25519::Pair>,
    call: RuntimeCall,
) -> ApplyExtrinsicResult {
    let dispatch_info = call.get_dispatch_info();
    let xt = construct_extrinsic(account, call)?;
    let xt_len = xt.encode().len();
    tracing::info!(
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

/// Build primary [`AgreementTerms`] for `owner` with the standard test shape.
fn primary_terms(
    owner: AccountId,
    max_bytes: u64,
    duration: u32,
    nonce: u64,
) -> pallet_storage_provider::AgreementTermsOf<Runtime> {
    AgreementTerms {
        owner,
        max_bytes,
        duration,
        price_per_byte: 0,
        // `valid_until` is checked against the pallet's relay-chain clock,
        // not the parachain height.
        valid_until: pallet_storage_provider::Pallet::<Runtime>::current_block()
            + <Runtime as pallet_storage_provider::Config>::RequestTimeout::get(),
        nonce,
        bucket_id: None,
        replica_params: None,
    }
}

/// Sign SCALE-encoded agreement terms with the provider keyring's sr25519 key.
///
/// `register_provider_for` stores the keyring's raw public key in the pallet,
/// so signatures produced here verify against that stored key.
fn sign_primary_terms(
    provider: Sr25519Keyring,
    terms: &pallet_storage_provider::AgreementTermsOf<Runtime>,
) -> sp_runtime::MultiSignature {
    let hash = sp_io::hashing::blake2_256(&terms.signing_payload());
    sp_runtime::MultiSignature::Sr25519(provider.pair().sign(&hash))
}

fn new_test_ext() -> sp_io::TestExternalities {
    sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap())
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

// ===============================
// Tests for the XCM functionality.
// ===============================

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

// ===============================
// Tests for the storage provider pallet.
// ===============================

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
fn should_deregister_provider() {
    new_test_ext().execute_with(|| {
        let account = Sr25519Keyring::Alice;
        let who: AccountId = account.to_account_id();
        let stake = default_stake();
        register_provider_for(account, stake);

        assert_eq!(Balances::reserved_balance(&who), stake);

        // Step 1: announce. Record stays, stake stays reserved, deregister_at stamped.
        assert_ok_ok(construct_and_apply_extrinsic(
            Some(account.pair()),
            RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::deregister_provider {}),
        ));
        let provider =
            StorageProvider::providers(&who).expect("provider must still exist after announce");
        let deregister_at = provider
            .deregister_at
            .expect("deregister_at must be set after announce");
        assert_eq!(Balances::reserved_balance(&who), stake);

        // Step 2: premature completion is rejected.
        let result = construct_and_apply_extrinsic(
            Some(account.pair()),
            RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::complete_deregister {}),
        );
        assert!(
            result.expect("extrinsic applies").is_err(),
            "complete_deregister before deregister_at must error"
        );

        // Step 3: fast-forward through the announcement period. Jump close to
        // the boundary with `set_clocks` (the period is `48 * RC_HOURS`
        // ≈ 28,800 blocks, far too many to iterate one-by-one) then cross it
        // via `advance_block` so the `on_finalize` / `on_initialize` hooks
        // fire at the maturing block.
        set_clocks(deregister_at.saturating_sub(1));
        advance_block();
        assert!(frame_system::Pallet::<Runtime>::block_number() >= deregister_at);

        // Step 4: complete. Record removed, stake unreserved.
        assert_ok_ok(construct_and_apply_extrinsic(
            Some(account.pair()),
            RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::complete_deregister {}),
        ));
        assert!(StorageProvider::providers(&who).is_none());
        assert_eq!(Balances::reserved_balance(&who), 0);
    });
}

#[test]
fn should_cancel_deregister_announcement() {
    new_test_ext().execute_with(|| {
        let account = Sr25519Keyring::Alice;
        let who: AccountId = account.to_account_id();
        let stake = default_stake();
        register_provider_for(account, stake);

        // Announce.
        assert_ok_ok(construct_and_apply_extrinsic(
            Some(account.pair()),
            RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::deregister_provider {}),
        ));
        let provider = StorageProvider::providers(&who).expect("provider must exist");
        assert!(provider.deregister_at.is_some());
        // Announce forces acceptance flags off.
        assert!(!provider.settings.accepting_primary);
        assert!(!provider.settings.accepting_extensions);

        // Advance a few blocks to make sure cancel still works within the window.
        advance_block();
        advance_block();

        // Cancel.
        assert_ok_ok(construct_and_apply_extrinsic(
            Some(account.pair()),
            RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::cancel_deregister {}),
        ));
        let provider =
            StorageProvider::providers(&who).expect("provider must still exist after cancel");
        // Cancel mirrors announce: deregister_at cleared, flags restored.
        assert!(provider.deregister_at.is_none());
        assert!(provider.settings.accepting_primary);
        assert!(provider.settings.accepting_extensions);
        // Stake stays reserved throughout — never went anywhere.
        assert_eq!(Balances::reserved_balance(&who), stake);
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

// ===============================
// Tests for the storage provider pallet via XCM.
// ===============================

#[test]
fn should_register_provider_via_xcm() {
    let account = Sr25519Keyring::Alice;
    let who: AccountId = account.to_account_id();
    let multiaddr: Vec<u8> = b"/ip4/127.0.0.1/tcp/3000".to_vec();
    let public_key = to_provider_public_key(account);

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
            // `default_stake` reads the storage-backed `MinProviderStake`, so it must
            // run inside the externalities provided by `execute_with`.
            let stake = default_stake();
            let register_provider_call =
                RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::register_provider {
                    multiaddr: multiaddr.clone().try_into().unwrap(),
                    public_key: public_key.clone(),
                    stake,
                });

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

#[test]
fn should_add_stake_via_xcm() {
    let alice_on_para = alice_on_sibling_parachain(2_000);
    let extra = 500 * UNIT;

    xcm_test_ext().execute_with(|| {
        // `default_stake` reads the storage-backed `MinProviderStake`, so it must
        // run inside the externalities provided by `execute_with`.
        let initial_stake = default_stake();
        let register_call =
            RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::register_provider {
                multiaddr: b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap(),
                public_key: to_provider_public_key(Sr25519Keyring::Alice),
                stake: initial_stake,
            });
        let add_stake_call =
            RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::add_stake {
                amount: extra,
            });

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

    xcm_test_ext().execute_with(|| {
        // `default_stake` reads the storage-backed `MinProviderStake`, so it must
        // run inside the externalities provided by `execute_with`.
        let stake = default_stake();
        let register_call =
            RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::register_provider {
                multiaddr: b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap(),
                public_key: to_provider_public_key(Sr25519Keyring::Alice),
                stake,
            });

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

    xcm_test_ext().execute_with(|| {
        // `default_stake` reads the storage-backed `MinProviderStake`, so it must
        // run inside the externalities provided by `execute_with`.
        let register_call =
            RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::register_provider {
                multiaddr: b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap(),
                public_key: to_provider_public_key(Sr25519Keyring::Alice),
                stake: default_stake(),
            });

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

// ===============================
// Tests for the Drive registry pallet.
// ===============================

/// Register `account` as a provider and configure it to accept primary
/// agreements with unlimited capacity and a wide duration window, so the
/// E2E flows can submit signed terms with arbitrary durations.
fn register_accepting_provider_for(account: Sr25519Keyring, stake: Balance) {
    register_provider_for(account, stake);
    assert_ok_ok(construct_and_apply_extrinsic(
        Some(account.pair()),
        RuntimeCall::StorageProvider(StorageProviderCall::<Runtime>::update_provider_settings {
            settings: pallet_storage_provider::ProviderSettings {
                min_duration: 1,
                max_duration: 1_000_000,
                price_per_byte: 0,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0, // Unlimited
            },
        }),
    ));
}

/// End-to-end drive lifecycle: provider setup → signed-terms create_drive →
/// share with a member → unshare → delete drive. Walks the complete
/// happy-path surface exposed by `pallet-drive-registry` so a single test
/// can spot regressions across the whole flow.
#[test]
fn drive_lifecycle_e2e() {
    new_test_ext().execute_with(|| {
        advance_block();

        let provider = Sr25519Keyring::Bob;
        let owner = Sr25519Keyring::Alice;
        let owner_id: AccountId = owner.to_account_id();
        let member_id: AccountId = Sr25519Keyring::Charlie.to_account_id();

        // 1. Provider registers + accepts primary agreements.
        register_accepting_provider_for(provider, default_stake());
        let _ = Balances::deposit_creating(&owner_id, 10 * UNIT);

        // 2. Owner redeems provider-signed terms to create the drive.
        let drive_id = pallet_drive_registry::NextDriveId::<Runtime>::get();
        let terms = primary_terms(owner_id.clone(), 1_000_000, 500, 1);
        let sig = sign_primary_terms(provider, &terms);

        assert_ok_ok(construct_and_apply_extrinsic(
            Some(owner.pair()),
            RuntimeCall::DriveRegistry(DriveRegistryCall::<Runtime>::create_drive {
                name: Some(b"My Drive".to_vec()),
                provider: provider.to_account_id(),
                terms,
                sig,
            }),
        ));

        let drive =
            pallet_drive_registry::Drives::<Runtime>::get(drive_id).expect("drive must be stored");
        assert_eq!(drive.owner, owner_id);
        assert_eq!(drive.max_capacity, 1_000_000);
        assert!(pallet_drive_registry::UserDrives::<Runtime>::get(&owner_id).contains(&drive_id));
        assert_eq!(
            pallet_drive_registry::NextDriveId::<Runtime>::get(),
            drive_id + 1
        );

        // 3. Share with Charlie as Reader, then revoke.
        assert_ok_ok(construct_and_apply_extrinsic(
            Some(owner.pair()),
            RuntimeCall::DriveRegistry(DriveRegistryCall::<Runtime>::share_drive {
                drive_id,
                member: member_id.clone(),
                role: storage_primitives::Role::Reader,
            }),
        ));
        System::assert_has_event(RuntimeEvent::DriveRegistry(
            pallet_drive_registry::Event::DriveShared {
                drive_id,
                member: member_id.clone(),
                role: storage_primitives::Role::Reader,
            },
        ));

        assert_ok_ok(construct_and_apply_extrinsic(
            Some(owner.pair()),
            RuntimeCall::DriveRegistry(DriveRegistryCall::<Runtime>::unshare_drive {
                drive_id,
                member: member_id.clone(),
            }),
        ));
        System::assert_has_event(RuntimeEvent::DriveRegistry(
            pallet_drive_registry::Event::DriveUnshared {
                drive_id,
                member: member_id,
            },
        ));

        // 4. Delete the drive.
        assert_ok_ok(construct_and_apply_extrinsic(
            Some(owner.pair()),
            RuntimeCall::DriveRegistry(DriveRegistryCall::<Runtime>::delete_drive { drive_id }),
        ));
        assert!(pallet_drive_registry::Drives::<Runtime>::get(drive_id).is_none());
        assert!(!pallet_drive_registry::UserDrives::<Runtime>::get(&owner_id).contains(&drive_id));
    });
}

// ===============================
// Tests for the Drive registry pallet via XCM.
// ===============================

/// End-to-end drive lifecycle dispatched via XCM from a sibling parachain:
/// the call's `Signed` origin is the *derived sovereign* of the
/// `(Parachain(N), AccountId32(Alice))` location, so the provider must sign
/// terms with `terms.owner = derived` (not Alice's keyring account).
#[test]
fn drive_lifecycle_via_xcm_e2e() {
    let alice_on_para = alice_on_sibling_parachain(4_000);
    let provider = Sr25519Keyring::Bob;
    let member_id: AccountId = Sr25519Keyring::Charlie.to_account_id();

    xcm_test_ext().execute_with(|| {
        // The dispatch origin is the sovereign `AccountId` derived from
        // Alice-on-para; that's who the drive will belong to.
        let derived: AccountId =
            LocationToAccountHelper::<AccountId, LocationToAccountId>::convert_location(
                alice_on_para.clone().into(),
            )
            .expect("Alice-on-para must convert to an account");

        // 1. Provider setup; fund the derived account for fees.
        register_accepting_provider_for(provider, default_stake());
        let _ = Balances::deposit_creating(&derived, 10 * UNIT);

        let drive_id = pallet_drive_registry::NextDriveId::<Runtime>::get();
        let fee: Asset = (Location::parent(), UNIT).into();

        // 2. Provider signs terms for the derived sovereign + XCM dispatches.
        let terms = primary_terms(derived.clone(), 1_000_000, 500, 1);
        let sig = sign_primary_terms(provider, &terms);
        let create_drive_call =
            RuntimeCall::DriveRegistry(DriveRegistryCall::<Runtime>::create_drive {
                name: Some(b"Sibling Drive".to_vec()),
                provider: provider.to_account_id(),
                terms,
                sig,
            });

        assert_ok!(
            RuntimeHelper::<Runtime, AllPalletsWithoutSystem>::execute_as_origin(
                (alice_on_para.clone(), OriginKind::SovereignAccount),
                create_drive_call,
                Some(fee.clone()),
            )
            .ensure_complete()
        );

        let drive =
            pallet_drive_registry::Drives::<Runtime>::get(drive_id).expect("drive must be stored");
        assert_eq!(drive.owner, derived);
        assert_eq!(drive.max_capacity, 1_000_000);
        assert!(pallet_drive_registry::UserDrives::<Runtime>::get(&derived).contains(&drive_id));

        // 3. Share with Charlie via XCM from the same origin.
        let share_call = RuntimeCall::DriveRegistry(DriveRegistryCall::<Runtime>::share_drive {
            drive_id,
            member: member_id.clone(),
            role: storage_primitives::Role::Reader,
        });
        assert_ok!(
            RuntimeHelper::<Runtime, AllPalletsWithoutSystem>::execute_as_origin(
                (alice_on_para.clone(), OriginKind::SovereignAccount),
                share_call,
                Some(fee.clone()),
            )
            .ensure_complete()
        );
        System::assert_has_event(RuntimeEvent::DriveRegistry(
            pallet_drive_registry::Event::DriveShared {
                drive_id,
                member: member_id.clone(),
                role: storage_primitives::Role::Reader,
            },
        ));

        // 4. Delete the drive via XCM.
        let delete_call =
            RuntimeCall::DriveRegistry(DriveRegistryCall::<Runtime>::delete_drive { drive_id });
        assert_ok!(
            RuntimeHelper::<Runtime, AllPalletsWithoutSystem>::execute_as_origin(
                (alice_on_para, OriginKind::SovereignAccount),
                delete_call,
                Some(fee),
            )
            .ensure_complete()
        );
        assert!(pallet_drive_registry::Drives::<Runtime>::get(drive_id).is_none());
    });
}

// ===============================
// Tests for the S3 registry pallet.
// ===============================

/// End-to-end S3 bucket lifecycle: provider setup → signed-terms
/// `create_s3_bucket` → put object metadata → delete object → delete bucket.
/// Combines the full happy-path object-store surface so a regression in any
/// step is caught by a single test.
#[test]
fn s3_bucket_lifecycle_e2e() {
    new_test_ext().execute_with(|| {
        advance_block();

        let provider = Sr25519Keyring::Bob;
        let user = Sr25519Keyring::Alice;
        let user_id: AccountId = user.to_account_id();

        // 1. Provider setup.
        register_accepting_provider_for(provider, default_stake());
        let _ = Balances::deposit_creating(&user_id, 10 * UNIT);

        // 2. Owner redeems provider-signed terms to create the S3 bucket.
        let s3_bucket_id = pallet_s3_registry::NextS3BucketId::<Runtime>::get();
        let terms = primary_terms(user_id.clone(), 1_000_000, 500, 1);
        let sig = sign_primary_terms(provider, &terms);
        assert_ok_ok(construct_and_apply_extrinsic(
            Some(user.pair()),
            RuntimeCall::S3Registry(S3RegistryCall::<Runtime>::create_s3_bucket {
                name: b"my-bucket".to_vec(),
                provider: provider.to_account_id(),
                terms,
                sig,
            }),
        ));

        let bucket = pallet_s3_registry::S3Buckets::<Runtime>::get(s3_bucket_id)
            .expect("bucket must be stored");
        assert_eq!(bucket.owner, user_id);
        assert_eq!(bucket.name.as_slice(), b"my-bucket");
        assert_eq!(bucket.object_count, 0);
        assert!(pallet_s3_registry::UserBuckets::<Runtime>::get(&user_id).contains(&s3_bucket_id));
        System::assert_has_event(RuntimeEvent::S3Registry(
            pallet_s3_registry::Event::S3BucketCreated {
                s3_bucket_id,
                name: b"my-bucket".to_vec(),
                layer0_bucket_id: bucket.layer0_bucket_id,
                owner: user_id.clone(),
            },
        ));

        // 3. Put an object, verify metadata + bucket stats updated.
        let cid = sp_core::H256::repeat_byte(0xAB);
        assert_ok_ok(construct_and_apply_extrinsic(
            Some(user.pair()),
            RuntimeCall::S3Registry(S3RegistryCall::<Runtime>::put_object_metadata {
                s3_bucket_id,
                key: b"photos/cat.jpg".to_vec(),
                cid,
                size: 1024,
                content_type: b"image/jpeg".to_vec(),
                user_metadata: vec![],
            }),
        ));
        let bucket = pallet_s3_registry::S3Buckets::<Runtime>::get(s3_bucket_id).unwrap();
        assert_eq!(bucket.object_count, 1);
        assert_eq!(bucket.total_size, 1024);
        let obj =
            S3Registry::get_object(s3_bucket_id, b"photos/cat.jpg").expect("object must be stored");
        assert_eq!(obj.cid, cid);
        assert_eq!(obj.size, 1024);

        // 4. Delete the object — bucket goes back to empty.
        assert_ok_ok(construct_and_apply_extrinsic(
            Some(user.pair()),
            RuntimeCall::S3Registry(S3RegistryCall::<Runtime>::delete_object_metadata {
                s3_bucket_id,
                key: b"photos/cat.jpg".to_vec(),
            }),
        ));
        let bucket = pallet_s3_registry::S3Buckets::<Runtime>::get(s3_bucket_id).unwrap();
        assert_eq!(bucket.object_count, 0);
        assert_eq!(bucket.total_size, 0);
        assert!(S3Registry::get_object(s3_bucket_id, b"photos/cat.jpg").is_none());

        // 5. Delete the empty bucket.
        assert_ok_ok(construct_and_apply_extrinsic(
            Some(user.pair()),
            RuntimeCall::S3Registry(S3RegistryCall::<Runtime>::delete_s3_bucket { s3_bucket_id }),
        ));
        assert!(pallet_s3_registry::S3Buckets::<Runtime>::get(s3_bucket_id).is_none());
        assert!(!pallet_s3_registry::UserBuckets::<Runtime>::get(&user_id).contains(&s3_bucket_id));
    });
}

// ===============================
// Tests for the S3 registry pallet via XCM.
// ===============================

/// End-to-end S3 bucket lifecycle dispatched via XCM from a sibling
/// parachain: each call's `Signed` origin is the *derived sovereign* of
/// `(Parachain(N), AccountId32(Alice))`, so the provider must sign terms
/// with `terms.owner = derived` (not Alice's keyring account).
#[test]
fn s3_bucket_lifecycle_via_xcm_e2e() {
    let alice_on_para = alice_on_sibling_parachain(5_000);
    let provider = Sr25519Keyring::Bob;

    xcm_test_ext().execute_with(|| {
        // Derive the sovereign account that XCM dispatch will use as `Signed`.
        let derived: AccountId =
            LocationToAccountHelper::<AccountId, LocationToAccountId>::convert_location(
                alice_on_para.clone().into(),
            )
            .expect("Alice-on-para must convert to an account");

        // 1. Provider setup + fund the derived account for fees and storage.
        register_accepting_provider_for(provider, default_stake());
        let _ = Balances::deposit_creating(&derived, 10 * UNIT);

        let s3_bucket_id = pallet_s3_registry::NextS3BucketId::<Runtime>::get();
        let fee: Asset = (Location::parent(), UNIT).into();

        // 2. Create the bucket: provider signs terms for `derived`, XCM
        //    dispatches the call from `alice_on_para`.
        let terms = primary_terms(derived.clone(), 1_000_000, 500, 1);
        let sig = sign_primary_terms(provider, &terms);
        let create_call = RuntimeCall::S3Registry(S3RegistryCall::<Runtime>::create_s3_bucket {
            name: b"sibling-bucket".to_vec(),
            provider: provider.to_account_id(),
            terms,
            sig,
        });
        assert_ok!(
            RuntimeHelper::<Runtime, AllPalletsWithoutSystem>::execute_as_origin(
                (alice_on_para.clone(), OriginKind::SovereignAccount),
                create_call,
                Some(fee.clone()),
            )
            .ensure_complete()
        );

        let bucket = pallet_s3_registry::S3Buckets::<Runtime>::get(s3_bucket_id)
            .expect("bucket must be stored");
        assert_eq!(bucket.owner, derived);
        assert_eq!(bucket.name.as_slice(), b"sibling-bucket");
        assert!(pallet_s3_registry::UserBuckets::<Runtime>::get(&derived).contains(&s3_bucket_id));

        // 3. Put an object via XCM.
        let cid = sp_core::H256::repeat_byte(0xAB);
        let put_call = RuntimeCall::S3Registry(S3RegistryCall::<Runtime>::put_object_metadata {
            s3_bucket_id,
            key: b"photos/cat.jpg".to_vec(),
            cid,
            size: 1024,
            content_type: b"image/jpeg".to_vec(),
            user_metadata: vec![],
        });
        assert_ok!(
            RuntimeHelper::<Runtime, AllPalletsWithoutSystem>::execute_as_origin(
                (alice_on_para.clone(), OriginKind::SovereignAccount),
                put_call,
                Some(fee.clone()),
            )
            .ensure_complete()
        );
        assert_eq!(
            S3Registry::get_object(s3_bucket_id, b"photos/cat.jpg")
                .unwrap()
                .cid,
            cid
        );

        // 4. Delete the object via XCM.
        let delete_obj_call =
            RuntimeCall::S3Registry(S3RegistryCall::<Runtime>::delete_object_metadata {
                s3_bucket_id,
                key: b"photos/cat.jpg".to_vec(),
            });
        assert_ok!(
            RuntimeHelper::<Runtime, AllPalletsWithoutSystem>::execute_as_origin(
                (alice_on_para.clone(), OriginKind::SovereignAccount),
                delete_obj_call,
                Some(fee.clone()),
            )
            .ensure_complete()
        );
        assert!(S3Registry::get_object(s3_bucket_id, b"photos/cat.jpg").is_none());

        // 5. Delete the empty bucket via XCM.
        let delete_bucket_call =
            RuntimeCall::S3Registry(S3RegistryCall::<Runtime>::delete_s3_bucket { s3_bucket_id });
        assert_ok!(
            RuntimeHelper::<Runtime, AllPalletsWithoutSystem>::execute_as_origin(
                (alice_on_para, OriginKind::SovereignAccount),
                delete_bucket_call,
                Some(fee),
            )
            .ensure_complete()
        );
        assert!(pallet_s3_registry::S3Buckets::<Runtime>::get(s3_bucket_id).is_none());
    });
}

//
// Genesis config presets
//

/// Recursively merge `patch` into `base` (same semantics as the chain-spec
/// `json_patch::merge` used by `GenesisBuilder::build_state`).
fn json_merge(base: &mut serde_json::Value, patch: serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(patch_map)) => {
            for (key, value) in patch_map {
                json_merge(
                    base_map.entry(key).or_insert(serde_json::Value::Null),
                    value,
                );
            }
        }
        (base_slot, patch_value) => *base_slot = patch_value,
    }
}

/// Build test externalities from a named genesis preset, exercising the same
/// JSON path as `GenesisBuilder::build_state`.
fn preset_ext(name: &str) -> sp_io::TestExternalities {
    let patch = storage_paseo_runtime::genesis_config_presets::get_preset(
        &sp_genesis_builder::PresetId::from(name),
    )
    .expect("preset must exist");
    let patch: serde_json::Value = serde_json::from_slice(&patch).expect("preset must be JSON");

    let mut base = serde_json::to_value(RuntimeGenesisConfig::default()).unwrap();
    json_merge(&mut base, patch);

    let config: RuntimeGenesisConfig =
        serde_json::from_value(base).expect("merged preset must deserialize");
    sp_io::TestExternalities::new(config.build_storage().unwrap())
}

#[test]
fn preset_names_contains_previewnet() {
    let names = storage_paseo_runtime::genesis_config_presets::preset_names();
    assert!(names.iter().any(|id| id.as_str()
        == storage_paseo_runtime::genesis_config_presets::PREVIEWNET_RUNTIME_PRESET));
}

#[test]
fn previewnet_preset_registers_default_provider() {
    preset_ext("previewnet").execute_with(|| {
        let alice = Sr25519Keyring::Alice.to_account_id();

        let provider = pallet_storage_provider::Providers::<Runtime>::get(&alice)
            .expect("PreviewNet default provider must be registered at genesis");
        assert_eq!(
            provider.multiaddr.to_vec(),
            b"/dns4/previewnet.substrate.dev/tcp/443/tls/http/http-path/web3-storage-provider"
        );
        assert_eq!(
            provider.public_key.to_vec(),
            Sr25519Keyring::Alice.to_raw_public_vec()
        );
        assert_eq!(provider.stake, 1_200_000_000_000 * UNIT);
        assert_eq!(provider.committed_bytes, 0);

        assert_eq!(provider.settings.min_duration, 100);
        assert_eq!(provider.settings.max_duration, 100_000);
        assert_eq!(provider.settings.price_per_byte, 1);
        assert!(provider.settings.accepting_primary);
        assert_eq!(provider.settings.replica_sync_price, None);
        assert!(provider.settings.accepting_extensions);
        assert_eq!(provider.settings.max_capacity, 1_099_511_627);

        assert_eq!(Balances::reserved_balance(&alice), 1_200_000_000_000 * UNIT);

        // The two genesis buckets from local_testnet are preserved.
        assert_eq!(pallet_storage_provider::NextBucketId::<Runtime>::get(), 2);

        assert_eq!(pallet_sudo::Key::<Runtime>::get(), Some(alice));
    });
}

#[test]
fn local_and_dev_presets_register_no_providers() {
    // Local flows (`just demo`, examples/papi) register the provider
    // themselves and would fail with `ProviderAlreadyRegistered` if these
    // presets pre-registered one.
    for preset in ["local_testnet", "development"] {
        preset_ext(preset).execute_with(|| {
            assert_eq!(
                pallet_storage_provider::Providers::<Runtime>::iter().count(),
                0,
                "preset {preset} must not register providers"
            );
        });
    }
}
