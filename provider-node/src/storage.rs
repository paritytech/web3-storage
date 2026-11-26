// SPDX-License-Identifier: GPL-3.0-only

//! Open the store and format the chain-mismatch error.

use crate::cli::StorageParams;
use provider_storage::StorageBackend;
use sp_core::H256;
use std::path::Path;
use std::sync::Arc;

/// Open the store and bind it to the chain with `genesis_hash`. Fails when
/// the store belongs to another chain.
pub(crate) fn open_storage(
    params: &StorageParams,
    genesis_hash: H256,
) -> Result<Arc<dyn StorageBackend>, Box<dyn std::error::Error>> {
    let backend = params.spec();
    tracing::info!("Storage backend: {backend}");
    backend
        .build(Some(genesis_hash))
        .map_err(|error| match error {
            provider_storage::Error::ChainMismatch {
                expected,
                connected,
            } => chain_mismatch_message(expected, connected, &params.storage_path).into(),
            error => error.into(),
        })
}

/// Operator message for a store that belongs to another chain.
pub(crate) fn chain_mismatch_message(
    expected: H256,
    connected: H256,
    storage_path: &Path,
) -> String {
    format!(
        "The data in {} belongs to chain {expected:?}, but the node is connected to chain \
         {connected:?}. Connect to the chain that wrote this data, or use another \
         --storage-path.",
        storage_path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::StorageBackendKind;

    #[test]
    fn open_storage_records_matches_and_rejects_another_chain() {
        let dir = tempfile::tempdir().unwrap();
        let params = StorageParams {
            storage_backend: StorageBackendKind::RocksDb,
            storage_path: dir.path().to_path_buf(),
        };
        let expected = H256::repeat_byte(0xaa);
        let connected = H256::repeat_byte(0xbb);

        // Each call reopens the store, as a node restart does.
        open_storage(&params, expected).unwrap();
        open_storage(&params, expected).unwrap();
        let err = open_storage(&params, connected)
            .err()
            .expect("another chain must be rejected")
            .to_string();
        assert_eq!(err, chain_mismatch_message(expected, connected, dir.path()));

        // The rejected chain did not replace the recorded one.
        open_storage(&params, expected).unwrap();
    }
}
