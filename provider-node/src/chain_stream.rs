//! Finalized-block stream with automatic reconnect.
//!
//! Coordinators (`AgreementCoordinator`, `CheckpointCoordinator`, …) react to on-chain
//! state and used to do so by polling. This helper turns the parachain's finalized-block
//! stream into a `tokio::sync::mpsc` channel, hides the reconnect bookkeeping behind a
//! single `next()` call, and marks the first tick after each (re)connect so consumers can
//! perform a one-time storage snapshot to cover anything that happened while disconnected.
//!
//! ```ignore
//! let mut stream = ChainStream::new(api.clone(), Duration::from_secs(6));
//! loop {
//!     tokio::select! {
//!         cmd = command_rx.recv() => { /* handle Stop, etc. */ }
//!         Some(tick) = stream.next() => {
//!             if tick.is_first_after_reconnect {
//!                 // snapshot at tick.block.hash()
//!             } else {
//!                 // react to tick.block.events().await
//!             }
//!         }
//!     }
//! }
//! ```

use sp_core::crypto::AccountId32;
use sp_core::H256;
use std::time::Duration;
use subxt::ext::scale_value::{At, Composite, Primitive, Value, ValueDef};
use subxt::{blocks::Block, OnlineClient, PolkadotConfig};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// A single yield from the stream — one finalized parachain block.
pub struct BlockTick {
    /// The finalized block.
    pub block: Block<PolkadotConfig, OnlineClient<PolkadotConfig>>,
    /// `true` for the first tick emitted after a (re)subscription. Consumers should
    /// perform a one-time storage snapshot at `block.hash()` to cover anything they
    /// might have missed.
    pub is_first_after_reconnect: bool,
}

impl BlockTick {
    /// Convenience accessor for the block hash.
    pub fn hash(&self) -> H256 {
        self.block.hash()
    }

    /// Convenience accessor for the block number.
    pub fn number(&self) -> u32 {
        self.block.number()
    }
}

/// Finalized-block stream with automatic reconnect.
///
/// Drop the stream to abort the background task.
pub struct ChainStream {
    rx: mpsc::Receiver<BlockTick>,
    task: JoinHandle<()>,
}

impl ChainStream {
    /// Spawn a background task that subscribes to finalized blocks and reconnects on
    /// failure with `reconnect_delay` between attempts.
    pub fn new(api: OnlineClient<PolkadotConfig>, reconnect_delay: Duration) -> Self {
        let (tx, rx) = mpsc::channel::<BlockTick>(16);
        let task = tokio::spawn(run(api, reconnect_delay, tx));
        Self { rx, task }
    }

    /// Await the next finalized block. Returns `None` only when the background task
    /// has exited (e.g. after the receiver is dropped).
    pub async fn next(&mut self) -> Option<BlockTick> {
        self.rx.recv().await
    }
}

impl Drop for ChainStream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run(
    api: OnlineClient<PolkadotConfig>,
    reconnect_delay: Duration,
    tx: mpsc::Sender<BlockTick>,
) {
    loop {
        let mut blocks = match api.blocks().subscribe_finalized().await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    "ChainStream: subscribe_finalized failed: {e}; retrying in {reconnect_delay:?}"
                );
                tokio::time::sleep(reconnect_delay).await;
                continue;
            }
        };

        let mut first = true;
        loop {
            match blocks.next().await {
                Some(Ok(block)) => {
                    let tick = BlockTick {
                        block,
                        is_first_after_reconnect: first,
                    };
                    first = false;
                    if tx.send(tick).await.is_err() {
                        // Receiver dropped — exit.
                        return;
                    }
                }
                Some(Err(e)) => {
                    tracing::warn!(
                        "ChainStream: block subscription error: {e}; reconnecting in {reconnect_delay:?}"
                    );
                    break;
                }
                None => {
                    tracing::warn!(
                        "ChainStream: block subscription ended; reconnecting in {reconnect_delay:?}"
                    );
                    break;
                }
            }
        }

        tokio::time::sleep(reconnect_delay).await;
    }
}

// ────────────────────────────────────────────────────────────────────────────
// SCALE-value decode helpers (mirror a subset of `storage_client::scale_decode`).
// Inlined here so provider-node doesn't need to depend on the storage-client crate
// (which would be cyclic — storage-client already depends on provider-node).
// ────────────────────────────────────────────────────────────────────────────

/// Read a named field as a `u64` (decoded from the underlying `u128`).
pub fn field_u64(fields: &Composite<u32>, name: &str) -> Option<u64> {
    fields.at(name)?.as_u128().map(|v| v as u64)
}

/// Read a named field as a `u32` (decoded from the underlying `u128`).
pub fn field_u32(fields: &Composite<u32>, name: &str) -> Option<u32> {
    fields.at(name)?.as_u128().map(|v| v as u32)
}

/// Read a named field and decode it as an [`AccountId32`].
pub fn field_account(fields: &Composite<u32>, name: &str) -> Option<AccountId32> {
    decode_account(fields.at(name)?)
}

/// Read a named field and decode it as an [`H256`].
pub fn field_h256(fields: &Composite<u32>, name: &str) -> Option<H256> {
    decode_h256(fields.at(name)?)
}

fn decode_account(v: &Value<u32>) -> Option<AccountId32> {
    let mut bytes = [0u8; 32];
    if collect_le_bytes(v, &mut bytes, 0) == 32 {
        Some(AccountId32::new(bytes))
    } else {
        None
    }
}

fn decode_h256(v: &Value<u32>) -> Option<H256> {
    let mut bytes = [0u8; 32];
    if collect_le_bytes(v, &mut bytes, 0) == 32 {
        Some(H256::from(bytes))
    } else {
        None
    }
}

fn collect_le_bytes(v: &Value<u32>, buf: &mut [u8; 32], offset: usize) -> usize {
    match &v.value {
        ValueDef::Primitive(Primitive::U128(n)) => {
            if offset < 32 {
                buf[offset] = *n as u8;
                offset + 1
            } else {
                offset
            }
        }
        ValueDef::Composite(Composite::Unnamed(items)) => {
            let mut pos = offset;
            for item in items {
                pos = collect_le_bytes(item, buf, pos);
            }
            pos
        }
        _ => offset,
    }
}
