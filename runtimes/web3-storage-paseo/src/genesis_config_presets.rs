//! Paseo Web3 Storage Parachain Runtime genesis config presets

use crate::paseo_constants::currency::UNIT;
use crate::*;
use alloc::{vec, vec::Vec};
use cumulus_primitives_core::ParaId;
use frame_support::build_struct_json_patch;
use pallet_storage_provider::{GenesisProvider, ProviderSettings};
use sp_genesis_builder::PresetId;
use sp_keyring::Sr25519Keyring;

// Parachain ID for Paseo Web3 Storage runtime
pub const WEB3_STORAGE_PARA_ID: ParaId = ParaId::new(1600);

/// Preset id for PreviewNet, consumed by the PPN repo's chain-spec script
/// (`chain-spec-builder create ... named-preset previewnet`).
pub const PREVIEWNET_RUNTIME_PRESET: &str = "previewnet";

/// The PreviewNet default provider (the PPN sidecar, running as //Alice).
///
/// Values mirror the provider registered on the live chain so a PreviewNet
/// wipe re-creates the same on-chain state. The multiaddr is the public,
/// TLS-terminated endpoint so on-chain provider discovery resolves to the
/// hosted provider rather than a client's own machine (issue #169). The
/// sidecar must be started with the matching `--public-multiaddr` so its
/// multiaddr-sync maintains this value instead of reverting it to the
/// bind-derived `/ip4/127.0.0.1/tcp/3333`.
fn previewnet_genesis_provider() -> GenesisProvider<Runtime> {
    GenesisProvider {
        account: Sr25519Keyring::Alice.to_account_id(),
        multiaddr:
            b"/dns4/previewnet.substrate.dev/tcp/443/tls/http/http-path/web3-storage-provider"
                .to_vec(),
        public_key: Sr25519Keyring::Alice.to_raw_public_vec(),
        stake: 1_200_000_000_000 * UNIT,
        settings: ProviderSettings {
            min_duration: 100,
            max_duration: 100_000,
            price_per_byte: 1,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 1_099_511_627,
        },
    }
}

fn storage_parachain_genesis(
    invulnerables: Vec<(AccountId, sp_consensus_aura::sr25519::AuthorityId)>,
    endowed_accounts: Vec<AccountId>,
    endowment: Balance,
    id: ParaId,
    sudo_account: Option<AccountId>,
    genesis_buckets: Vec<(AccountId, u32)>,
    genesis_providers: Vec<GenesisProvider<Runtime>>,
) -> serde_json::Value {
    build_struct_json_patch!(RuntimeGenesisConfig {
        balances: BalancesConfig {
            balances: endowed_accounts
                .iter()
                .cloned()
                .map(|k| (k, endowment))
                .collect(),
        },
        parachain_info: ParachainInfoConfig { parachain_id: id },
        collator_selection: CollatorSelectionConfig {
            invulnerables: invulnerables.iter().cloned().map(|(acc, _)| acc).collect(),
            candidacy_bond: EXISTENTIAL_DEPOSIT * 16,
        },
        session: SessionConfig {
            keys: invulnerables
                .into_iter()
                .map(|(acc, aura)| {
                    (
                        acc.clone(),          // account id
                        acc,                  // validator id
                        SessionKeys { aura }, // session keys
                    )
                })
                .collect(),
        },
        polkadot_xcm: PolkadotXcmConfig {
            safe_xcm_version: Some(xcm::latest::VERSION)
        },
        sudo: SudoConfig { key: sudo_account },
        storage_provider: StorageProviderConfig {
            buckets: genesis_buckets,
            providers: genesis_providers,
        },
    })
}

/// Shared base for the `local_testnet` and `previewnet` presets: Alice+Bob
/// collators, all well-known endowed accounts, the two genesis buckets and
/// Alice as sudo. The presets differ only in their genesis providers, which is
/// passed in.
fn testnet_genesis(genesis_providers: Vec<GenesisProvider<Runtime>>) -> serde_json::Value {
    storage_parachain_genesis(
        // initial collators.
        vec![
            (
                Sr25519Keyring::Alice.to_account_id(),
                Sr25519Keyring::Alice.public().into(),
            ),
            (
                Sr25519Keyring::Bob.to_account_id(),
                Sr25519Keyring::Bob.public().into(),
            ),
        ],
        Sr25519Keyring::well_known()
            .map(|k| k.to_account_id())
            .collect(),
        UNIT * 10_000_000_000_000,
        WEB3_STORAGE_PARA_ID,
        // Sudo
        Some(Sr25519Keyring::Alice.to_account_id()),
        // Genesis buckets: creates bucket_id=0 and bucket_id=1 (admin, min_providers)
        vec![
            (Sr25519Keyring::Bob.to_account_id(), 1),
            (Sr25519Keyring::Bob.to_account_id(), 1),
        ],
        genesis_providers,
    )
}

/// Provides the JSON representation of predefined genesis config for given `id`.
pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
    let patch = match id.as_ref() {
        // No genesis providers: local flows (`just demo`, examples/papi)
        // register the provider themselves.
        sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET => testnet_genesis(vec![]),
        sp_genesis_builder::DEV_RUNTIME_PRESET => storage_parachain_genesis(
            // initial collators.
            vec![(
                Sr25519Keyring::Alice.to_account_id(),
                Sr25519Keyring::Alice.public().into(),
            )],
            vec![
                Sr25519Keyring::Alice.to_account_id(),
                Sr25519Keyring::Bob.to_account_id(),
                Sr25519Keyring::AliceStash.to_account_id(),
                Sr25519Keyring::BobStash.to_account_id(),
            ],
            UNIT * 10_000_000_000_000,
            WEB3_STORAGE_PARA_ID,
            // Sudo
            Some(Sr25519Keyring::Alice.to_account_id()),
            // Genesis buckets: creates bucket_id=0 and bucket_id=1 (admin, min_providers)
            vec![
                (Sr25519Keyring::Bob.to_account_id(), 1),
                (Sr25519Keyring::Bob.to_account_id(), 1),
            ],
            // No genesis providers: local flows (`just demo`, examples/papi)
            // register the provider themselves.
            vec![],
        ),
        PREVIEWNET_RUNTIME_PRESET => testnet_genesis(vec![previewnet_genesis_provider()]),
        _ => return None,
    };

    Some(
        serde_json::to_string(&patch)
            .expect("serialization to json is expected to work. qed.")
            .into_bytes(),
    )
}

/// List of supported presets.
pub fn preset_names() -> Vec<PresetId> {
    vec![
        PresetId::from(sp_genesis_builder::DEV_RUNTIME_PRESET),
        PresetId::from(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET),
        PresetId::from(PREVIEWNET_RUNTIME_PRESET),
    ]
}
