// SPDX-License-Identifier: Apache-2.0

//! Chain-state coordinator: keeps the provider node's view of the runtime in
//! sync via a finalized-block subscription.
//!
//! [`ChainState`] is the single source of truth for all on-chain state the
//! provider node needs at runtime:
//! - [`ChainState::current_anchor_block`] — the pallet's anchor block (the
//!   clock all on-chain durations use), read via its runtime API.
//! - [`ChainState::constants`] — pallet constants, read on connect and after
//!   a runtime upgrade.
//! - [`ChainState::provider_info`] — full provider registration info.
//!
//! [`ChainStateCoordinator`] is the **only writer** for all three fields.  It
//! drives a finalized-block subscription on its own chain connection in a
//! reconnect loop; on every relevant provider event it re-fetches the full
//! `ProviderInfo` so `committed_bytes`, `stake`, and all settings stay
//! current — no field-patching, no partial updates, no second writer.
//!
//! It also broadcasts bucket-membership changes as
//! [`BlockEvent::BucketMembershipChanged`], so the membership cache can drop
//! stale authorization on its own rather than being told to.

mod chain;

pub use chain::{
    BlockContents, ChainConnection, ChainFollower, ChainStateChainClient, FinalizedBlock,
    FinalizedBlocks,
};

use provider_events::{BlockEvent, BlockEventTx};
use provider_types::ChainClientError;
pub use provider_types::{ChainState, PalletConstants};
use sp_runtime::AccountId32;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

// ── Error ─────────────────────────────────────────────────────────────────────

/// Why the coordinator's reconnect loop dropped a connection. The loop logs it
/// and reconnects.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// A chain call failed.
    #[error(transparent)]
    ChainClient(#[from] ChainClientError),

    /// A step of the reconnect loop did not finish within its budget.
    #[error("{what} timed out after {secs}s")]
    Timeout {
        /// The step that timed out.
        what: &'static str,
        /// The budget, in seconds.
        secs: u64,
    },
}

/// Run `fut` under `budget`, mapping expiry to [`Error::Timeout`] naming `what`.
///
/// Every wait in the reconnect loop goes through this: an operation that can
/// hang forever (e.g. a wedged smoldot backend with no timeout of its own)
/// must turn into an `Err` so the loop can rebuild the connection instead of
/// hanging with a stale handle still published.
async fn with_timeout<T>(
    what: &'static str,
    budget: Duration,
    fut: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    match tokio::time::timeout(budget, fut).await {
        Ok(result) => result,
        Err(_) => Err(Error::Timeout {
            what,
            secs: budget.as_secs(),
        }),
    }
}

// ── ChainStateCoordinator ─────────────────────────────────────────────────────

/// Builds and starts the live chain-state synchronisation for a single provider.
///
/// Start with [`ChainStateCoordinator::start`]; keep the returned
/// [`ChainStateCoordinatorHandle`] alive for the duration of the server.
pub struct ChainStateCoordinator {
    /// Opens the chain connection and publishes it to the node's other chain
    /// consumers.
    follower: Arc<dyn ChainFollower>,
    provider_account: AccountId32,
    chain_state: Arc<ChainState>,
    /// Fan-out of decoded per-block events to the background coordinators.
    events_tx: BlockEventTx,
}

impl ChainStateCoordinator {
    /// Coordinator for `provider_account` that writes `chain_state` and sends
    /// decoded block events on `events_tx`. Call [`Self::start`] to run it.
    pub fn new(
        follower: Arc<dyn ChainFollower>,
        provider_account: AccountId32,
        chain_state: Arc<ChainState>,
        events_tx: BlockEventTx,
    ) -> Self {
        Self {
            follower,
            provider_account,
            chain_state,
            events_tx,
        }
    }

    /// Spawn the coordinator and return immediately.
    ///
    /// The spawned task connects to the chain, follows finalized blocks, and
    /// reconnects automatically: a chain that is unreachable at startup or that
    /// drops the connection later is retried with a fixed backoff instead of
    /// taking the coordinator down. Runs until the returned handle is dropped,
    /// [`ChainStateCoordinatorHandle::stop`] is called, or a connection reports
    /// [`ChainClientError::ChainMismatch`]; the handle's
    /// [`fatal_error`](ChainStateCoordinatorHandle::fatal_error) returns that error.
    pub fn start(self) -> ChainStateCoordinatorHandle {
        let task = tokio::spawn(self.run());
        ChainStateCoordinatorHandle { task }
    }

    /// Reconnect loop: (re)connect and follow finalized blocks, sleeping
    /// [`RECONNECT_DELAY`] between attempts so an unreachable chain doesn't spin.
    /// Returns only on [`ChainClientError::ChainMismatch`], which retrying cannot fix.
    async fn run(self) -> ChainClientError {
        const RECONNECT_DELAY: Duration = Duration::from_secs(5);

        loop {
            match self.connect_and_follow().await {
                Ok(()) => tracing::warn!(
                    "chain-state coordinator: block stream ended; reconnecting in {}s",
                    RECONNECT_DELAY.as_secs()
                ),
                Err(Error::ChainClient(e @ ChainClientError::ChainMismatch { .. })) => return e,
                Err(e) => tracing::warn!(
                    "chain-state coordinator: {e}; reconnecting in {}s",
                    RECONNECT_DELAY.as_secs()
                ),
            }
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    }

    /// Connect to the chain, bootstrap initial state, then drive the finalized-block
    /// stream until it ends or stalls. Returns `Err` if connecting or bootstrapping
    /// fails; `Ok(())` if the stream terminates — either way the caller reconnects.
    async fn connect_and_follow(&self) -> Result<(), Error> {
        /// Budget for building a cold connection and subscribing to its
        /// finalized blocks. On the light transport, `connect` awaits
        /// smoldot's peer discovery and warp sync with no timeout of its own,
        /// so a wedged light client would otherwise hang here forever — with
        /// the previous (dead) handle still published to consumers — and the
        /// reconnect loop could never rebuild it. Generous because killing a
        /// slow warp sync throws its progress away.
        const CONNECT_TIMEOUT: Duration = Duration::from_secs(300);

        let connection = with_timeout("Connecting to the chain", CONNECT_TIMEOUT, async {
            Ok(self.follower.connect().await?)
        })
        .await?;
        self.follow(connection).await
    }

    /// Bootstrap state from the connection and follow its finalized blocks
    /// until the stream ends or stalls. Returns `Err` if a state read times
    /// out. Split from
    /// [`Self::connect_and_follow`] so tests can drive the full pipeline over
    /// a mock connection.
    async fn follow(&self, connection: ChainConnection) -> Result<(), Error> {
        /// How long without a finalized block before the connection is treated
        /// as dead and rebuilt. Finality can pause briefly (session boundaries,
        /// backend resubscriptions), so this is several times the block time;
        /// a genuinely stalled stream otherwise hangs forever with no error.
        /// The connection is already warp-synced by `connect`, so the first
        /// block gets the same budget as every other.
        const STALL_TIMEOUT: Duration = Duration::from_secs(60);
        /// Budget for one read of the chain state, on connect or after a
        /// runtime upgrade: ordinary RPC round-trips on
        /// an already-synced connection, but on the light client they have no
        /// timeout of their own and a wedged backend would otherwise hang the
        /// reconnect loop forever.
        const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(60);

        let ChainConnection { mut blocks, client } = connection;
        let chain = client.as_ref();
        tracing::info!("chain-state coordinator: connected; following finalized blocks");

        // Read the state on connect, so a restarted node that was already
        // registered has its provider_info without waiting for the next
        // relevant event. Events emitted while the stream was down were
        // missed, so the other coordinators re-scan chain state.
        with_timeout(
            "Chain bootstrap",
            BOOTSTRAP_TIMEOUT,
            self.read_chain_state(chain),
        )
        .await?;

        // Makes the loop read the state again at the next block. Set at a
        // block that stored a new runtime: the following block is the first
        // one the new runtime executes and the one its migrations run in.
        let mut read_again = false;

        loop {
            let block = match tokio::time::timeout(STALL_TIMEOUT, blocks.next()).await {
                Ok(Some(block)) => block,
                Ok(None) => break,
                Err(_) => {
                    tracing::warn!(
                        "chain-state coordinator: no finalized block for {}s; rebuilding connection",
                        STALL_TIMEOUT.as_secs()
                    );
                    break;
                }
            };
            // A failed anchor read keeps the previous value.
            if let Some(anchor_block) = block.anchor_block {
                self.chain_state
                    .current_anchor_block
                    .store(anchor_block, std::sync::atomic::Ordering::Relaxed);
            }

            let Some(contents) = block.contents else {
                escalate_block_read_failure(&self.events_tx, block.number);
                // The block may have stored a new runtime, and its events are
                // lost for every coordinator.
                read_again = true;
                continue;
            };

            let runtime_upgraded = contents
                .events
                .iter()
                .any(|event| matches!(event, BlockEvent::RuntimeUpgraded { .. }));

            // Fan out the coordinator-relevant events. Send failures just mean
            // no coordinator is subscribed.
            for event in contents.events {
                let _ = self.events_tx.send(event);
            }

            if read_again {
                tracing::info!(
                    "chain-state coordinator: reading chain state again at block {}",
                    block.number
                );
                with_timeout(
                    "Chain state read",
                    BOOTSTRAP_TIMEOUT,
                    self.read_chain_state(chain),
                )
                .await?;
            } else {
                refresh_if_relevant_event(
                    chain,
                    &self.chain_state,
                    &self.provider_account,
                    &contents.lifecycle,
                    block.number,
                )
                .await;
            }
            read_again = runtime_upgraded;
        }

        Ok(())
    }

    /// Read the pallet constants and the provider's registration, then tell
    /// the other coordinators to reconcile with a
    /// [`BlockEvent::Resubscribed`]. Never `Err`; the `Result` is for
    /// [`with_timeout`].
    async fn read_chain_state(&self, chain: &dyn ChainStateChainClient) -> Result<(), Error> {
        sync_constants(chain, &self.chain_state).await;
        refresh_provider_state(chain, &self.chain_state, &self.provider_account).await;
        let _ = self.events_tx.send(BlockEvent::Resubscribed {
            at_block: self
                .chain_state
                .current_anchor_block
                .load(std::sync::atomic::Ordering::Relaxed),
        });
        Ok(())
    }
}

// ── state synchronisation (chain-client-agnostic) ─────────────────────────────

/// Fetch the `StorageProvider::RequestTimeout` runtime constant and store it in
/// `chain_state.constants`. Called on each (re)connect and after a runtime
/// upgrade. Logs at warn if absent so operators notice a metadata problem
/// rather than silent 503s.
pub async fn sync_constants(chain: &dyn ChainStateChainClient, chain_state: &ChainState) {
    match chain.fetch_request_timeout().await {
        Ok(Some(timeout)) => {
            *chain_state.constants.write() = Some(PalletConstants {
                request_timeout: timeout,
            });
            tracing::debug!("chain-state coordinator: RequestTimeout = {timeout}");
        }
        Ok(None) => tracing::warn!(
            "chain-state coordinator: RequestTimeout constant absent from runtime metadata;"
        ),
        Err(e) => tracing::warn!("chain-state coordinator: failed to fetch RequestTimeout: {e}"),
    }
}

/// Re-fetch `ProviderInfo` from chain and update `chain_state`.
///
/// Called on each (re)connect, after a runtime upgrade and on every relevant
/// provider event.
pub async fn refresh_provider_state(
    chain: &dyn ChainStateChainClient,
    chain_state: &ChainState,
    provider_account: &AccountId32,
) {
    match chain.get_provider_info(provider_account).await {
        Ok(Some(info)) => {
            *chain_state.provider_info.write() = Some(info);
        }
        // Provider is not (or no longer) registered on chain.
        Ok(None) => {
            *chain_state.provider_info.write() = None;
            tracing::debug!("chain-state coordinator: provider not registered on chain");
        }
        Err(e) => tracing::warn!("chain-state coordinator: failed to fetch provider info: {e}"),
    }
}

/// Refresh provider state iff `provider_account` is among the providers of a
/// block's lifecycle events ([`BlockContents::lifecycle`]). Collapsing
/// multiple events in one block to a single refresh is correct:
/// [`refresh_provider_state`] always reads the latest chain state, so no
/// intermediate event is "missed".
pub async fn refresh_if_relevant_event(
    chain: &dyn ChainStateChainClient,
    chain_state: &ChainState,
    provider_account: &AccountId32,
    event_providers: &[AccountId32],
    block_number: u32,
) {
    if event_providers.contains(provider_account) {
        tracing::debug!(
            "chain-state coordinator: provider event in block {block_number}, refreshing state"
        );
        refresh_provider_state(chain, chain_state, provider_account).await;
    }
}

// ── ChainStateCoordinatorHandle ───────────────────────────────────────────────

/// Handle to the coordinator task. Dropping it leaves the task running; call
/// [`stop`](Self::stop) to shut it down.
pub struct ChainStateCoordinatorHandle {
    task: JoinHandle<ChainClientError>,
}

impl ChainStateCoordinatorHandle {
    /// Resolves when the coordinator stops on a fatal error. `Err` if the
    /// task panicked.
    pub async fn fatal_error(self) -> Result<ChainClientError, tokio::task::JoinError> {
        self.task.await
    }

    /// Stop the coordinator. Aborting the loop drops the stream, which aborts the
    /// underlying block subscription.
    pub async fn stop(self) {
        tracing::info!("chain-state coordinator: stopped");
        self.task.abort();
        let _ = self.task.await;
    }
}

/// A finalized block's handle or events could not be fetched at all, so its
/// membership events (if any) are lost rather than merely dropped one at a
/// time - the same "no bucket id left to invalidate" situation an undecodable
/// event escalates to, and for the same reason: a plain `continue` here would
/// let a revoked member keep authorizing for a full `--auth-cache-ttl` on a
/// node that is otherwise connected and healthy.
fn escalate_block_read_failure(events_tx: &BlockEventTx, block_number: u32) {
    let _ = events_tx.send(BlockEvent::MembershipScopeUnknown {
        at_block: block_number,
    });
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use provider_types::{ProviderInfo, ProviderSettings, ProviderStats};
    use sp_runtime::testing::H256;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn provider_account() -> AccountId32 {
        AccountId32::new([7u8; 32])
    }

    #[test]
    fn escalate_block_read_failure_broadcasts_membership_scope_unknown() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(4);
        escalate_block_read_failure(&tx, 99);
        assert!(matches!(
            rx.try_recv(),
            Ok(BlockEvent::MembershipScopeUnknown { at_block: 99 })
        ));
    }

    fn sample_provider_info() -> ProviderInfo {
        ProviderInfo {
            multiaddr: "/ip4/1.2.3.4/tcp/3333".to_string(),
            public_key: vec![1u8; 32],
            stake: 1_000,
            committed_bytes: 500,
            settings: ProviderSettings {
                min_duration: 10,
                max_duration: 100,
                price_per_byte: 5,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 10_000,
                min_agreement_bytes: 0,
            },
            stats: ProviderStats {
                agreements_total: 3,
                challenges_failed: 1,
                ..Default::default()
            },
            deregister_at: None,
        }
    }

    #[test]
    fn chain_state_defaults_to_unknown() {
        let cs = ChainState::default();
        assert_eq!(cs.current_anchor_block.load(Ordering::Relaxed), 0);
        assert!(cs.constants.read().is_none());
        assert!(cs.provider_info.read().is_none());
    }

    #[test]
    fn chain_state_current_anchor_block_round_trips() {
        let cs = ChainState::default();
        cs.current_anchor_block.store(42, Ordering::Relaxed);
        assert_eq!(cs.current_anchor_block.load(Ordering::Relaxed), 42);
    }

    #[test]
    fn chain_state_provider_info_round_trips() {
        let cs = ChainState::default();
        *cs.provider_info.write() = Some(sample_provider_info());
        let guard = cs.provider_info.read();
        let info = guard.as_ref().unwrap();
        assert_eq!(info.settings.price_per_byte, 5);
        assert_eq!(info.committed_bytes, 500);
        assert_eq!(info.multiaddr, "/ip4/1.2.3.4/tcp/3333");
    }

    #[test]
    fn chain_state_constants_round_trips() {
        let cs = ChainState::default();
        assert!(cs.constants.read().is_none());
        *cs.constants.write() = Some(PalletConstants {
            request_timeout: 100,
        });
        assert_eq!(cs.constants.read().as_ref().unwrap().request_timeout, 100);
    }

    #[tokio::test(start_paused = true)]
    async fn with_timeout_expiry_maps_to_error() {
        // Paused time auto-advances past the budget instantly.
        let err = with_timeout(
            "Chain bootstrap",
            Duration::from_secs(5),
            std::future::pending::<Result<(), Error>>(),
        )
        .await
        .expect_err("a never-ready future must hit the budget");
        assert!(
            err.to_string()
                .contains("Chain bootstrap timed out after 5s"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn with_timeout_passes_results_through() {
        let ok = with_timeout("op", Duration::from_secs(1), async { Ok::<_, Error>(7) })
            .await
            .expect("value passes through");
        assert_eq!(ok, 7);

        let err = with_timeout("op", Duration::from_secs(1), async {
            Err::<(), _>(ChainClientError::query("op", "inner failure").into())
        })
        .await
        .expect_err("inner error passes through");
        assert!(
            err.to_string().contains("inner failure"),
            "unexpected error: {err}"
        );
    }

    // ── mock chain follower ────────────────────────────────────────────────
    //
    // These drive [`ChainStateCoordinator`] without a chain: a mock
    // [`ChainStateChainClient`] answers the bootstrap reads, and a plain
    // iterator yields already-decoded blocks. provider-node tests the subxt
    // decoding.

    /// [`ChainStateChainClient`] returning fixed answers.
    #[derive(Default)]
    struct MockChainClient {
        provider_info: Option<ProviderInfo>,
        request_timeout: Option<u32>,
    }

    #[async_trait]
    impl ChainStateChainClient for MockChainClient {
        async fn get_provider_info(
            &self,
            _who: &AccountId32,
        ) -> Result<Option<ProviderInfo>, ChainClientError> {
            Ok(self.provider_info.clone())
        }

        async fn fetch_request_timeout(&self) -> Result<Option<u32>, ChainClientError> {
            Ok(self.request_timeout)
        }
    }

    #[async_trait]
    impl FinalizedBlocks for std::vec::IntoIter<FinalizedBlock> {
        async fn next(&mut self) -> Option<FinalizedBlock> {
            Iterator::next(self)
        }
    }

    /// [`ChainFollower`] for tests that call [`ChainStateCoordinator::follow`]
    /// directly; `connect()` must not run.
    struct NeverConnectFollower;

    #[async_trait]
    impl ChainFollower for NeverConnectFollower {
        async fn connect(&self) -> Result<ChainConnection, ChainClientError> {
            unreachable!("connect() must not be called when driving follow() directly")
        }
    }

    /// [`ChainFollower`] whose `connect()` always fails, counting attempts.
    struct AlwaysFailFollower {
        attempts: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ChainFollower for AlwaysFailFollower {
        async fn connect(&self) -> Result<ChainConnection, ChainClientError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(ChainClientError::query(
                "chain connection",
                "mock connect failure",
            ))
        }
    }

    /// [`ChainFollower`] whose `connect()` always reports a chain mismatch,
    /// counting attempts.
    struct MismatchFollower {
        attempts: Arc<AtomicUsize>,
        expected: H256,
        connected: H256,
    }

    #[async_trait]
    impl ChainFollower for MismatchFollower {
        async fn connect(&self) -> Result<ChainConnection, ChainClientError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(ChainClientError::ChainMismatch {
                expected: self.expected,
                connected: self.connected,
            })
        }
    }

    /// A readable block with the given anchor block and events.
    fn block(
        number: u32,
        anchor_block: Option<u32>,
        events: Vec<BlockEvent>,
        lifecycle: Vec<AccountId32>,
    ) -> FinalizedBlock {
        FinalizedBlock {
            number,
            anchor_block,
            contents: Some(BlockContents { events, lifecycle }),
        }
    }

    /// Run [`ChainStateCoordinator::follow`] over `blocks` until the stream
    /// ends. Returns the resulting chain state and every event broadcast.
    async fn run_follow(
        client: MockChainClient,
        blocks: Vec<FinalizedBlock>,
    ) -> (Arc<ChainState>, Vec<BlockEvent>) {
        let chain_state = Arc::new(ChainState::default());
        let (events_tx, mut events_rx) = tokio::sync::broadcast::channel(64);
        let coordinator = ChainStateCoordinator::new(
            Arc::new(NeverConnectFollower),
            provider_account(),
            chain_state.clone(),
            events_tx,
        );

        coordinator
            .follow(ChainConnection {
                blocks: Box::new(blocks.into_iter()),
                client: Box::new(client),
            })
            .await
            .expect("follow runs to stream end");

        let mut events = Vec::new();
        while let Ok(event) = events_rx.try_recv() {
            events.push(event);
        }
        (chain_state, events)
    }

    #[tokio::test]
    async fn follow_processes_finalized_blocks_and_provider_events() {
        let account = provider_account();
        let info = sample_provider_info();
        let client = MockChainClient {
            provider_info: Some(info.clone()),
            request_timeout: Some(100),
        };
        let blocks = vec![block(
            42,
            Some(4242),
            vec![BlockEvent::ChallengeCreated {
                deadline: 777,
                index: 3,
                bucket_id: 9,
                provider: account.clone(),
            }],
            vec![account.clone()],
        )];

        let (chain_state, events) = run_follow(client, blocks).await;

        assert_eq!(
            chain_state.current_anchor_block.load(Ordering::Relaxed),
            4242
        );
        let stored = chain_state.provider_info.read();
        let stored = stored.as_ref().expect("provider info synced from chain");
        assert_eq!(stored.stake, info.stake);
        assert!(chain_state.constants.read().is_some());

        assert!(
            events
                .iter()
                .any(|e| matches!(e, BlockEvent::Resubscribed { .. })),
            "follow should broadcast Resubscribed"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                BlockEvent::ChallengeCreated {
                    deadline: 777,
                    index: 3,
                    bucket_id: 9,
                    provider,
                } if *provider == account
            )),
            "follow should forward the block's decoded events"
        );
    }

    #[tokio::test]
    async fn follow_broadcasts_membership_changes() {
        // Duplicates included: invalidation is idempotent, and the fan-out
        // does not deduplicate.
        let blocks = vec![block(
            1,
            Some(1),
            [9, 7, 7, 8]
                .map(|bucket_id| BlockEvent::BucketMembershipChanged { bucket_id })
                .to_vec(),
            vec![],
        )];

        let (_chain_state, events) = run_follow(MockChainClient::default(), blocks).await;

        let changed_buckets: Vec<_> = events
            .into_iter()
            .filter_map(|event| match event {
                BlockEvent::BucketMembershipChanged { bucket_id } => Some(bucket_id),
                _ => None,
            })
            .collect();
        assert_eq!(changed_buckets, vec![9, 7, 7, 8]);
    }

    /// An unreadable block escalates instead of being dropped, its anchor
    /// block is still stored, and the blocks behind it are still processed.
    #[tokio::test]
    async fn follow_continues_past_an_unreadable_block() {
        let account = provider_account();
        let challenge = BlockEvent::ChallengeCreated {
            deadline: 777,
            index: 3,
            bucket_id: 9,
            provider: account.clone(),
        };
        let blocks = vec![
            // Unreadable, but the anchor read succeeded.
            FinalizedBlock {
                number: 10,
                anchor_block: Some(200),
                contents: None,
            },
            // No anchor, so the 200 above remains.
            block(11, None, vec![challenge], vec![]),
        ];

        let (chain_state, events) = run_follow(MockChainClient::default(), blocks).await;

        assert_eq!(
            chain_state.current_anchor_block.load(Ordering::Relaxed),
            200
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, BlockEvent::MembershipScopeUnknown { at_block: 10 })),
            "an unreadable block must escalate MembershipScopeUnknown"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                BlockEvent::ChallengeCreated { provider, .. } if *provider == account
            )),
            "the block after an unreadable one must still be processed"
        );
    }

    #[tokio::test]
    async fn follow_keeps_the_previous_anchor_block_when_a_read_fails() {
        let blocks = vec![
            block(1, Some(100), vec![], vec![]),
            block(2, None, vec![], vec![]),
        ];

        let (chain_state, _events) = run_follow(MockChainClient::default(), blocks).await;

        assert_eq!(
            chain_state.current_anchor_block.load(Ordering::Relaxed),
            100
        );
    }

    /// A chain that delivers `blocks` in order and then no more. `None` is a
    /// block whose events could not be read.
    #[derive(Clone)]
    struct ScriptedChain {
        blocks: Vec<Option<Vec<BlockEvent>>>,
        delivered_blocks: Arc<AtomicUsize>,
        connects: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ChainFollower for ScriptedChain {
        async fn connect(&self) -> Result<ChainConnection, ChainClientError> {
            self.connects.fetch_add(1, Ordering::SeqCst);
            Ok(ChainConnection {
                blocks: Box::new(self.clone()),
                client: Box::new(self.clone()),
            })
        }
    }

    #[async_trait]
    impl FinalizedBlocks for ScriptedChain {
        async fn next(&mut self) -> Option<FinalizedBlock> {
            let index = self.delivered_blocks.load(Ordering::SeqCst);
            let Some(events) = self.blocks.get(index).cloned() else {
                return std::future::pending().await;
            };
            self.delivered_blocks.fetch_add(1, Ordering::SeqCst);
            let number = index as u32 + 1;
            Some(FinalizedBlock {
                number,
                anchor_block: Some(number),
                contents: events.map(|events| BlockContents {
                    events,
                    lifecycle: vec![],
                }),
            })
        }
    }

    #[async_trait]
    impl ChainStateChainClient for ScriptedChain {
        async fn get_provider_info(
            &self,
            _who: &AccountId32,
        ) -> Result<Option<ProviderInfo>, ChainClientError> {
            Ok(Some(sample_provider_info()))
        }

        /// The number of the last delivered block, so a test can tell at
        /// which block the coordinator last read the chain state.
        async fn fetch_request_timeout(&self) -> Result<Option<u32>, ChainClientError> {
            Ok(Some(self.delivered_blocks.load(Ordering::SeqCst) as u32))
        }
    }

    /// Run the coordinator over a [`ScriptedChain`] until it has processed
    /// every block, on one connection. Returns the events it sent and the
    /// number of the block at which it last read the chain state (0 is the
    /// read on connect).
    async fn run_scripted(blocks: Vec<Option<Vec<BlockEvent>>>) -> (Vec<BlockEvent>, u32) {
        let chain_state = Arc::new(ChainState::default());
        let (events_tx, mut events_rx) = tokio::sync::broadcast::channel(16);
        let connects = Arc::new(AtomicUsize::new(0));
        let handle = ChainStateCoordinator::new(
            Arc::new(ScriptedChain {
                blocks,
                delivered_blocks: Arc::new(AtomicUsize::new(0)),
                connects: connects.clone(),
            }),
            provider_account(),
            chain_state.clone(),
            events_tx,
        )
        .start();

        // The clock is paused, so this returns once the coordinator waits for
        // a block that never arrives. It is shorter than the reconnect delay.
        tokio::time::sleep(Duration::from_secs(1)).await;
        handle.stop().await;

        assert_eq!(connects.load(Ordering::SeqCst), 1);
        let mut events = Vec::new();
        while let Ok(event) = events_rx.try_recv() {
            events.push(event);
        }
        let last_read = chain_state
            .constants
            .read()
            .as_ref()
            .expect("the coordinator read the constants")
            .request_timeout;
        (events, last_read)
    }

    /// Block 1 stores a new runtime. The coordinator reads the state again at
    /// block 2, where the migrations ran, without reconnecting, and sends
    /// `Resubscribed`. It does not read again at block 3.
    #[tokio::test(start_paused = true)]
    async fn run_reads_chain_state_again_at_the_block_after_a_runtime_upgrade() {
        let (events, last_read) = run_scripted(vec![
            Some(vec![BlockEvent::RuntimeUpgraded { at_block: 1 }]),
            Some(vec![]),
            Some(vec![]),
        ])
        .await;

        assert!(
            matches!(
                events.as_slice(),
                [
                    BlockEvent::Resubscribed { .. },
                    BlockEvent::RuntimeUpgraded { at_block: 1 },
                    BlockEvent::Resubscribed { .. },
                ]
            ),
            "unexpected events: {events:?}"
        );
        assert_eq!(last_read, 2);
    }

    /// The events of block 1 could not be read, so it may have stored a new
    /// runtime. The coordinator reads the state again at block 2. Block 2
    /// stores a new runtime, so the coordinator also reads at block 3, and
    /// not at block 4.
    #[tokio::test(start_paused = true)]
    async fn run_reads_chain_state_again_after_an_unreadable_block() {
        let (events, last_read) = run_scripted(vec![
            None,
            Some(vec![BlockEvent::RuntimeUpgraded { at_block: 2 }]),
            Some(vec![]),
            Some(vec![]),
        ])
        .await;

        assert!(
            matches!(
                events.as_slice(),
                [
                    BlockEvent::Resubscribed { .. },
                    BlockEvent::MembershipScopeUnknown { at_block: 1 },
                    BlockEvent::RuntimeUpgraded { at_block: 2 },
                    BlockEvent::Resubscribed { .. },
                    BlockEvent::Resubscribed { .. },
                ]
            ),
            "unexpected events: {events:?}"
        );
        assert_eq!(last_read, 3);
    }

    #[tokio::test(start_paused = true)]
    async fn run_retries_after_a_failed_connect() {
        const RECONNECT_DELAY: Duration = Duration::from_secs(5);

        let attempts = Arc::new(AtomicUsize::new(0));
        let follower: Arc<dyn ChainFollower> = Arc::new(AlwaysFailFollower {
            attempts: attempts.clone(),
        });
        let chain_state = ChainState::default();
        let coordinator = ChainStateCoordinator::new(
            follower,
            provider_account(),
            Arc::new(chain_state),
            tokio::sync::broadcast::channel(16).0,
        );
        let handle = coordinator.start();

        // Drive the paused clock through several reconnect cycles: each
        // failed `connect()` is followed by a `RECONNECT_DELAY` sleep before
        // `run()` tries again.
        for _ in 0..3 {
            tokio::time::advance(RECONNECT_DELAY).await;
        }

        assert!(
            attempts.load(Ordering::SeqCst) >= 2,
            "run() must retry connect() after a failure, got {} attempts",
            attempts.load(Ordering::SeqCst)
        );

        handle.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn run_stops_on_a_chain_mismatch_after_one_attempt() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let expected = H256::repeat_byte(1);
        let connected = H256::repeat_byte(2);
        let follower: Arc<dyn ChainFollower> = Arc::new(MismatchFollower {
            attempts: attempts.clone(),
            expected,
            connected,
        });
        let handle = ChainStateCoordinator::new(
            follower,
            provider_account(),
            Arc::new(ChainState::default()),
            tokio::sync::broadcast::channel(16).0,
        )
        .start();

        let fatal = tokio::time::timeout(Duration::from_secs(60), handle.fatal_error())
            .await
            .expect("the coordinator must stop on a chain mismatch")
            .expect("the coordinator task must not panic");

        assert!(matches!(
            fatal,
            ChainClientError::ChainMismatch { expected: e, connected: c }
                if e == expected && c == connected
        ));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
