//! Storage Parachain Runtime genesis config presets

use crate::*;
use alloc::{vec, vec::Vec};
use cumulus_primitives_core::ParaId;
use frame_support::build_struct_json_patch;
use sp_genesis_builder::PresetId;
use sp_keyring::Sr25519Keyring;

/// The parachain ID for this parachain.
pub const PARA_ID: ParaId = ParaId::new(4000);

fn storage_parachain_genesis(
    invulnerables: Vec<(AccountId, sp_consensus_aura::sr25519::AuthorityId)>,
    endowed_accounts: Vec<AccountId>,
    endowment: Balance,
    id: ParaId,
    sudo_account: Option<AccountId>,
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
    })
}

/// Provides the JSON representation of predefined genesis config for given `id`.
pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
    let patch = match id.as_ref() {
        sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET => storage_parachain_genesis(
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
            UNIT * 1_000_000,
            PARA_ID,
            // Sudo
            Some(Sr25519Keyring::Alice.to_account_id()),
        ),
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
            UNIT * 1_000_000,
            PARA_ID,
            // Sudo
            Some(Sr25519Keyring::Alice.to_account_id()),
        ),
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
    ]
}
