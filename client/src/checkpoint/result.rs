// SPDX-License-Identifier: Apache-2.0

//! Result of attempting to submit a checkpoint.

use sp_core::H256;
use sp_runtime::AccountId32;

/// Result of attempting to submit a checkpoint.
#[derive(Clone, Debug)]
pub enum CheckpointResult {
    /// Checkpoint submitted successfully.
    Submitted {
        /// Block hash where the transaction was included.
        block_hash: H256,
        /// Providers whose signatures were included.
        signers: Vec<AccountId32>,
    },
    /// Not enough providers agreed (below threshold).
    InsufficientConsensus {
        /// Number of agreeing providers.
        agreeing: usize,
        /// Number required to meet threshold.
        required: usize,
        /// Providers with different data.
        disagreements: Vec<(AccountId32, H256)>,
    },
    /// All providers were unreachable.
    ProvidersUnreachable {
        /// List of unreachable providers.
        providers: Vec<AccountId32>,
    },
    /// No providers found for this bucket.
    NoProviders,
    /// Transaction submission failed.
    TransactionFailed {
        /// Error message.
        error: String,
    },
}
