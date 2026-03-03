//! WebSocket Event Subscription Module
//!
//! Provides real-time subscription to storage provider events from the blockchain,
//! including checkpoints, challenges, and agreement lifecycle events.
//!
//! # Example
//!
//! ```no_run
//! use storage_client::event_subscription::{EventSubscriber, EventFilter, StorageEvent};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create subscriber
//! let mut subscriber = EventSubscriber::connect("ws://localhost:2222").await?;
//!
//! // Subscribe to checkpoint events for a specific bucket
//! subscriber.set_filter(EventFilter::bucket(1));
//!
//! // Process events
//! while let Some(event) = subscriber.next_event().await {
//!     match event {
//!         StorageEvent::BucketCheckpointed { bucket_id, mmr_root, .. } => {
//!             println!("Checkpoint for bucket {}: {:?}", bucket_id, mmr_root);
//!         }
//!         StorageEvent::ChallengeCreated { challenge_id, provider, .. } => {
//!             println!("New challenge {} for provider {:?}", challenge_id.1, provider);
//!         }
//!         _ => {}
//!     }
//! }
//! # Ok(())
//! # }
//! ```

use crate::ClientError;
use futures::Stream;
use sp_core::H256;
use sp_runtime::AccountId32;
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use storage_primitives::BucketId;
use subxt::{OnlineClient, PolkadotConfig};
use tokio::sync::mpsc;

// ============================================================================
// Event Types
// ============================================================================

/// Challenge identifier: (deadline_block, index)
pub type ChallengeId = (u32, u16);

/// Storage provider events from the blockchain.
#[derive(Clone, Debug)]
pub enum StorageEvent {
    // ========================================================================
    // Checkpoint Events
    // ========================================================================
    /// A bucket checkpoint was submitted successfully.
    BucketCheckpointed {
        bucket_id: BucketId,
        mmr_root: H256,
        start_seq: u64,
        leaf_count: u64,
        providers: Vec<AccountId32>,
        block_hash: H256,
        block_number: u32,
    },

    // ========================================================================
    // Challenge Events
    // ========================================================================
    /// A new challenge was created against a provider.
    ChallengeCreated {
        challenge_id: ChallengeId,
        bucket_id: BucketId,
        provider: AccountId32,
        challenger: AccountId32,
        respond_by: u32,
        block_hash: H256,
        block_number: u32,
    },

    /// A challenge was successfully defended by the provider.
    ChallengeDefended {
        challenge_id: ChallengeId,
        provider: AccountId32,
        response_time_blocks: u32,
        challenger_cost: u128,
        provider_cost: u128,
        block_hash: H256,
        block_number: u32,
    },

    /// A provider was slashed for failing to defend a challenge.
    ChallengeSlashed {
        challenge_id: ChallengeId,
        provider: AccountId32,
        slashed_amount: u128,
        challenger_reward: u128,
        block_hash: H256,
        block_number: u32,
    },

    // ========================================================================
    // Provider Events
    // ========================================================================
    /// A new storage provider was registered.
    ProviderRegistered {
        provider: AccountId32,
        stake: u128,
        block_hash: H256,
        block_number: u32,
    },

    /// A provider was added to a bucket as primary provider.
    ProviderAddedToBucket {
        bucket_id: BucketId,
        provider: AccountId32,
        block_hash: H256,
        block_number: u32,
    },

    /// A primary provider was removed from a bucket.
    PrimaryProviderRemoved {
        bucket_id: BucketId,
        provider: AccountId32,
        reason: String,
        block_hash: H256,
        block_number: u32,
    },

    // ========================================================================
    // Agreement Events
    // ========================================================================
    /// A storage agreement was requested.
    AgreementRequested {
        bucket_id: BucketId,
        provider: AccountId32,
        requester: AccountId32,
        max_bytes: u64,
        payment_locked: u128,
        duration: u32,
        block_hash: H256,
        block_number: u32,
    },

    /// A storage agreement was accepted.
    AgreementAccepted {
        bucket_id: BucketId,
        provider: AccountId32,
        expires_at: u32,
        block_hash: H256,
        block_number: u32,
    },

    /// A storage agreement ended.
    AgreementEnded {
        bucket_id: BucketId,
        provider: AccountId32,
        payment_to_provider: u128,
        burned: u128,
        block_hash: H256,
        block_number: u32,
    },

    // ========================================================================
    // Bucket Events
    // ========================================================================
    /// A new bucket was created.
    BucketCreated {
        bucket_id: BucketId,
        admin: AccountId32,
        block_hash: H256,
        block_number: u32,
    },

    /// A bucket was frozen.
    BucketFrozen {
        bucket_id: BucketId,
        frozen_start_seq: u64,
        block_hash: H256,
        block_number: u32,
    },

    /// A bucket was deleted.
    BucketDeleted {
        bucket_id: BucketId,
        block_hash: H256,
        block_number: u32,
    },

    // ========================================================================
    // Replica Events
    // ========================================================================
    /// A replica synced its data.
    ReplicaSynced {
        bucket_id: BucketId,
        provider: AccountId32,
        mmr_root: H256,
        sync_payment: u128,
        block_hash: H256,
        block_number: u32,
    },

    // ========================================================================
    // Generic/Unknown Events
    // ========================================================================
    /// An unknown or unparsed event from the StorageProvider pallet.
    Unknown {
        pallet: String,
        variant: String,
        block_hash: H256,
        block_number: u32,
    },
}

impl StorageEvent {
    /// Get the bucket ID associated with this event, if any.
    pub fn bucket_id(&self) -> Option<BucketId> {
        match self {
            StorageEvent::BucketCheckpointed { bucket_id, .. } => Some(*bucket_id),
            StorageEvent::ChallengeCreated { bucket_id, .. } => Some(*bucket_id),
            StorageEvent::ProviderAddedToBucket { bucket_id, .. } => Some(*bucket_id),
            StorageEvent::PrimaryProviderRemoved { bucket_id, .. } => Some(*bucket_id),
            StorageEvent::AgreementRequested { bucket_id, .. } => Some(*bucket_id),
            StorageEvent::AgreementAccepted { bucket_id, .. } => Some(*bucket_id),
            StorageEvent::AgreementEnded { bucket_id, .. } => Some(*bucket_id),
            StorageEvent::BucketCreated { bucket_id, .. } => Some(*bucket_id),
            StorageEvent::BucketFrozen { bucket_id, .. } => Some(*bucket_id),
            StorageEvent::BucketDeleted { bucket_id, .. } => Some(*bucket_id),
            StorageEvent::ReplicaSynced { bucket_id, .. } => Some(*bucket_id),
            _ => None,
        }
    }

    /// Get the provider associated with this event, if any.
    pub fn provider(&self) -> Option<&AccountId32> {
        match self {
            StorageEvent::BucketCheckpointed { providers, .. } => providers.first(),
            StorageEvent::ChallengeCreated { provider, .. } => Some(provider),
            StorageEvent::ChallengeDefended { provider, .. } => Some(provider),
            StorageEvent::ChallengeSlashed { provider, .. } => Some(provider),
            StorageEvent::ProviderRegistered { provider, .. } => Some(provider),
            StorageEvent::ProviderAddedToBucket { provider, .. } => Some(provider),
            StorageEvent::PrimaryProviderRemoved { provider, .. } => Some(provider),
            StorageEvent::AgreementRequested { provider, .. } => Some(provider),
            StorageEvent::AgreementAccepted { provider, .. } => Some(provider),
            StorageEvent::AgreementEnded { provider, .. } => Some(provider),
            StorageEvent::ReplicaSynced { provider, .. } => Some(provider),
            _ => None,
        }
    }

    /// Get the block hash where this event occurred.
    pub fn block_hash(&self) -> H256 {
        match self {
            StorageEvent::BucketCheckpointed { block_hash, .. } => *block_hash,
            StorageEvent::ChallengeCreated { block_hash, .. } => *block_hash,
            StorageEvent::ChallengeDefended { block_hash, .. } => *block_hash,
            StorageEvent::ChallengeSlashed { block_hash, .. } => *block_hash,
            StorageEvent::ProviderRegistered { block_hash, .. } => *block_hash,
            StorageEvent::ProviderAddedToBucket { block_hash, .. } => *block_hash,
            StorageEvent::PrimaryProviderRemoved { block_hash, .. } => *block_hash,
            StorageEvent::AgreementRequested { block_hash, .. } => *block_hash,
            StorageEvent::AgreementAccepted { block_hash, .. } => *block_hash,
            StorageEvent::AgreementEnded { block_hash, .. } => *block_hash,
            StorageEvent::BucketCreated { block_hash, .. } => *block_hash,
            StorageEvent::BucketFrozen { block_hash, .. } => *block_hash,
            StorageEvent::BucketDeleted { block_hash, .. } => *block_hash,
            StorageEvent::ReplicaSynced { block_hash, .. } => *block_hash,
            StorageEvent::Unknown { block_hash, .. } => *block_hash,
        }
    }

    /// Get the block number where this event occurred.
    pub fn block_number(&self) -> u32 {
        match self {
            StorageEvent::BucketCheckpointed { block_number, .. } => *block_number,
            StorageEvent::ChallengeCreated { block_number, .. } => *block_number,
            StorageEvent::ChallengeDefended { block_number, .. } => *block_number,
            StorageEvent::ChallengeSlashed { block_number, .. } => *block_number,
            StorageEvent::ProviderRegistered { block_number, .. } => *block_number,
            StorageEvent::ProviderAddedToBucket { block_number, .. } => *block_number,
            StorageEvent::PrimaryProviderRemoved { block_number, .. } => *block_number,
            StorageEvent::AgreementRequested { block_number, .. } => *block_number,
            StorageEvent::AgreementAccepted { block_number, .. } => *block_number,
            StorageEvent::AgreementEnded { block_number, .. } => *block_number,
            StorageEvent::BucketCreated { block_number, .. } => *block_number,
            StorageEvent::BucketFrozen { block_number, .. } => *block_number,
            StorageEvent::BucketDeleted { block_number, .. } => *block_number,
            StorageEvent::ReplicaSynced { block_number, .. } => *block_number,
            StorageEvent::Unknown { block_number, .. } => *block_number,
        }
    }

    /// Check if this is a checkpoint-related event.
    pub fn is_checkpoint_event(&self) -> bool {
        matches!(self, StorageEvent::BucketCheckpointed { .. })
    }

    /// Check if this is a challenge-related event.
    pub fn is_challenge_event(&self) -> bool {
        matches!(
            self,
            StorageEvent::ChallengeCreated { .. }
                | StorageEvent::ChallengeDefended { .. }
                | StorageEvent::ChallengeSlashed { .. }
        )
    }

    /// Check if this is an agreement-related event.
    pub fn is_agreement_event(&self) -> bool {
        matches!(
            self,
            StorageEvent::AgreementRequested { .. }
                | StorageEvent::AgreementAccepted { .. }
                | StorageEvent::AgreementEnded { .. }
        )
    }
}

// ============================================================================
// Event Filter
// ============================================================================

/// Filter for selecting which events to receive.
#[derive(Clone, Debug, Default)]
pub struct EventFilter {
    /// Only include events for these bucket IDs (empty = all buckets).
    pub bucket_ids: HashSet<BucketId>,
    /// Only include events for these providers (empty = all providers).
    pub providers: HashSet<AccountId32>,
    /// Include checkpoint events.
    pub include_checkpoints: bool,
    /// Include challenge events.
    pub include_challenges: bool,
    /// Include agreement events.
    pub include_agreements: bool,
    /// Include bucket lifecycle events.
    pub include_bucket_lifecycle: bool,
    /// Include provider events.
    pub include_provider_events: bool,
    /// Include replica events.
    pub include_replica_events: bool,
    /// Include unknown/unparsed events.
    pub include_unknown: bool,
}

impl EventFilter {
    /// Create a filter that matches all events.
    pub fn all() -> Self {
        Self {
            bucket_ids: HashSet::new(),
            providers: HashSet::new(),
            include_checkpoints: true,
            include_challenges: true,
            include_agreements: true,
            include_bucket_lifecycle: true,
            include_provider_events: true,
            include_replica_events: true,
            include_unknown: true,
        }
    }

    /// Create a filter for a specific bucket.
    pub fn bucket(bucket_id: BucketId) -> Self {
        let mut filter = Self::all();
        filter.bucket_ids.insert(bucket_id);
        filter
    }

    /// Create a filter for a specific provider.
    pub fn provider(provider: AccountId32) -> Self {
        let mut filter = Self::all();
        filter.providers.insert(provider);
        filter
    }

    /// Create a filter for checkpoint events only.
    pub fn checkpoints_only() -> Self {
        Self {
            include_checkpoints: true,
            ..Default::default()
        }
    }

    /// Create a filter for challenge events only.
    pub fn challenges_only() -> Self {
        Self {
            include_challenges: true,
            ..Default::default()
        }
    }

    /// Add a bucket ID to the filter.
    pub fn with_bucket(mut self, bucket_id: BucketId) -> Self {
        self.bucket_ids.insert(bucket_id);
        self
    }

    /// Add a provider to the filter.
    pub fn with_provider(mut self, provider: AccountId32) -> Self {
        self.providers.insert(provider);
        self
    }

    /// Check if an event matches this filter.
    pub fn matches(&self, event: &StorageEvent) -> bool {
        // Check bucket ID filter
        if !self.bucket_ids.is_empty() {
            if let Some(bucket_id) = event.bucket_id() {
                if !self.bucket_ids.contains(&bucket_id) {
                    return false;
                }
            }
        }

        // Check provider filter
        if !self.providers.is_empty() {
            if let Some(provider) = event.provider() {
                if !self.providers.contains(provider) {
                    return false;
                }
            }
        }

        // Check event type filter
        match event {
            StorageEvent::BucketCheckpointed { .. } => self.include_checkpoints,
            StorageEvent::ChallengeCreated { .. }
            | StorageEvent::ChallengeDefended { .. }
            | StorageEvent::ChallengeSlashed { .. } => self.include_challenges,
            StorageEvent::AgreementRequested { .. }
            | StorageEvent::AgreementAccepted { .. }
            | StorageEvent::AgreementEnded { .. } => self.include_agreements,
            StorageEvent::BucketCreated { .. }
            | StorageEvent::BucketFrozen { .. }
            | StorageEvent::BucketDeleted { .. } => self.include_bucket_lifecycle,
            StorageEvent::ProviderRegistered { .. }
            | StorageEvent::ProviderAddedToBucket { .. }
            | StorageEvent::PrimaryProviderRemoved { .. } => self.include_provider_events,
            StorageEvent::ReplicaSynced { .. } => self.include_replica_events,
            StorageEvent::Unknown { .. } => self.include_unknown,
        }
    }
}

// ============================================================================
// Event Subscriber
// ============================================================================

/// WebSocket subscriber for blockchain events.
pub struct EventSubscriber {
    /// Subxt API client.
    api: OnlineClient<PolkadotConfig>,
    /// Event filter.
    filter: EventFilter,
    /// Whether the subscriber is running.
    running: Arc<AtomicBool>,
    /// Event receiver channel.
    event_rx: Option<mpsc::Receiver<StorageEvent>>,
    /// Background task handle.
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl EventSubscriber {
    /// Connect to a blockchain node and create a subscriber.
    pub async fn connect(ws_url: &str) -> Result<Self, ClientError> {
        let api = OnlineClient::<PolkadotConfig>::from_url(ws_url)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to connect: {e}")))?;

        Ok(Self {
            api,
            filter: EventFilter::all(),
            running: Arc::new(AtomicBool::new(false)),
            event_rx: None,
            task_handle: None,
        })
    }

    /// Set the event filter.
    pub fn set_filter(&mut self, filter: EventFilter) {
        self.filter = filter;
    }

    /// Get a reference to the current filter.
    pub fn filter(&self) -> &EventFilter {
        &self.filter
    }

    /// Start the event subscription.
    ///
    /// This begins listening for finalized blocks and extracting events.
    pub async fn start(&mut self) -> Result<(), ClientError> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        let (event_tx, event_rx) = mpsc::channel(1000);
        self.event_rx = Some(event_rx);
        self.running.store(true, Ordering::SeqCst);

        let api = self.api.clone();
        let filter = self.filter.clone();
        let running = self.running.clone();

        let handle = tokio::spawn(async move {
            if let Err(e) =
                Self::run_subscription_loop(api, filter, event_tx, running.clone()).await
            {
                tracing::error!("Event subscription loop error: {}", e);
            }
            running.store(false, Ordering::SeqCst);
        });

        self.task_handle = Some(handle);
        Ok(())
    }

    /// Stop the event subscription.
    pub async fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.task_handle.take() {
            let _ = handle.await;
        }
        self.event_rx = None;
    }

    /// Check if the subscription is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Get the next event, blocking until one is available.
    ///
    /// Returns None if the subscription has stopped.
    pub async fn next_event(&mut self) -> Option<StorageEvent> {
        if let Some(rx) = &mut self.event_rx {
            rx.recv().await
        } else {
            None
        }
    }

    /// Try to get the next event without blocking.
    ///
    /// Returns None if no event is available or subscription has stopped.
    pub fn try_next_event(&mut self) -> Option<StorageEvent> {
        if let Some(rx) = &mut self.event_rx {
            rx.try_recv().ok()
        } else {
            None
        }
    }

    /// Run the subscription loop.
    async fn run_subscription_loop(
        api: OnlineClient<PolkadotConfig>,
        filter: EventFilter,
        event_tx: mpsc::Sender<StorageEvent>,
        running: Arc<AtomicBool>,
    ) -> Result<(), ClientError> {
        let mut block_sub = api
            .blocks()
            .subscribe_finalized()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to subscribe to blocks: {e}")))?;

        while running.load(Ordering::SeqCst) {
            match block_sub.next().await {
                Some(Ok(block)) => {
                    let block_hash = H256::from_slice(block.hash().as_ref());
                    let block_number = block.number();

                    // Get events from this block
                    match block.events().await {
                        Ok(events) => {
                            for event_result in events.iter() {
                                match event_result {
                                    Ok(event) => {
                                        // Only process StorageProvider pallet events
                                        if event.pallet_name() == "StorageProvider" {
                                            if let Some(storage_event) =
                                                Self::parse_event(&event, block_hash, block_number)
                                            {
                                                if filter.matches(&storage_event)
                                                    && event_tx.send(storage_event).await.is_err()
                                                {
                                                    // Channel closed, stop
                                                    return Ok(());
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("Failed to decode event: {}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to get block events: {}", e);
                        }
                    }
                }
                Some(Err(e)) => {
                    tracing::error!("Block subscription error: {}", e);
                    // Try to continue
                }
                None => {
                    // Stream ended
                    break;
                }
            }
        }

        Ok(())
    }

    /// Parse a subxt event into a StorageEvent.
    fn parse_event(
        event: &subxt::events::EventDetails<PolkadotConfig>,
        block_hash: H256,
        block_number: u32,
    ) -> Option<StorageEvent> {
        let variant = event.variant_name();

        // Parse based on event variant name
        // Note: In production, you would use proper decoding based on runtime metadata
        match variant {
            "BucketCheckpointed" => {
                // Try to extract fields from event bytes
                // This is a simplified version - proper implementation would decode SCALE
                Some(StorageEvent::BucketCheckpointed {
                    bucket_id: 0, // Would be decoded from event
                    mmr_root: H256::zero(),
                    start_seq: 0,
                    leaf_count: 0,
                    providers: vec![],
                    block_hash,
                    block_number,
                })
            }
            "ChallengeCreated" => Some(StorageEvent::ChallengeCreated {
                challenge_id: (0, 0),
                bucket_id: 0,
                provider: AccountId32::new([0u8; 32]),
                challenger: AccountId32::new([0u8; 32]),
                respond_by: 0,
                block_hash,
                block_number,
            }),
            "ChallengeDefended" => Some(StorageEvent::ChallengeDefended {
                challenge_id: (0, 0),
                provider: AccountId32::new([0u8; 32]),
                response_time_blocks: 0,
                challenger_cost: 0,
                provider_cost: 0,
                block_hash,
                block_number,
            }),
            "ChallengeSlashed" => Some(StorageEvent::ChallengeSlashed {
                challenge_id: (0, 0),
                provider: AccountId32::new([0u8; 32]),
                slashed_amount: 0,
                challenger_reward: 0,
                block_hash,
                block_number,
            }),
            "ProviderRegistered" => Some(StorageEvent::ProviderRegistered {
                provider: AccountId32::new([0u8; 32]),
                stake: 0,
                block_hash,
                block_number,
            }),
            "ProviderAddedToBucket" => Some(StorageEvent::ProviderAddedToBucket {
                bucket_id: 0,
                provider: AccountId32::new([0u8; 32]),
                block_hash,
                block_number,
            }),
            "AgreementRequested" => Some(StorageEvent::AgreementRequested {
                bucket_id: 0,
                provider: AccountId32::new([0u8; 32]),
                requester: AccountId32::new([0u8; 32]),
                max_bytes: 0,
                payment_locked: 0,
                duration: 0,
                block_hash,
                block_number,
            }),
            "AgreementAccepted" => Some(StorageEvent::AgreementAccepted {
                bucket_id: 0,
                provider: AccountId32::new([0u8; 32]),
                expires_at: 0,
                block_hash,
                block_number,
            }),
            "AgreementEnded" => Some(StorageEvent::AgreementEnded {
                bucket_id: 0,
                provider: AccountId32::new([0u8; 32]),
                payment_to_provider: 0,
                burned: 0,
                block_hash,
                block_number,
            }),
            "BucketCreated" => Some(StorageEvent::BucketCreated {
                bucket_id: 0,
                admin: AccountId32::new([0u8; 32]),
                block_hash,
                block_number,
            }),
            "BucketFrozen" => Some(StorageEvent::BucketFrozen {
                bucket_id: 0,
                frozen_start_seq: 0,
                block_hash,
                block_number,
            }),
            "BucketDeleted" => Some(StorageEvent::BucketDeleted {
                bucket_id: 0,
                block_hash,
                block_number,
            }),
            "ReplicaSynced" => Some(StorageEvent::ReplicaSynced {
                bucket_id: 0,
                provider: AccountId32::new([0u8; 32]),
                mmr_root: H256::zero(),
                sync_payment: 0,
                block_hash,
                block_number,
            }),
            _ => Some(StorageEvent::Unknown {
                pallet: "StorageProvider".to_string(),
                variant: variant.to_string(),
                block_hash,
                block_number,
            }),
        }
    }
}

// ============================================================================
// Event Stream
// ============================================================================

/// A stream of storage events.
pub struct EventStream {
    subscriber: EventSubscriber,
}

impl EventStream {
    /// Create a new event stream.
    pub async fn new(ws_url: &str, filter: EventFilter) -> Result<Self, ClientError> {
        let mut subscriber = EventSubscriber::connect(ws_url).await?;
        subscriber.set_filter(filter);
        subscriber.start().await?;
        Ok(Self { subscriber })
    }

    /// Create a stream for checkpoint events only.
    pub async fn checkpoints(ws_url: &str) -> Result<Self, ClientError> {
        Self::new(ws_url, EventFilter::checkpoints_only()).await
    }

    /// Create a stream for challenge events only.
    pub async fn challenges(ws_url: &str) -> Result<Self, ClientError> {
        Self::new(ws_url, EventFilter::challenges_only()).await
    }

    /// Create a stream for a specific bucket.
    pub async fn for_bucket(ws_url: &str, bucket_id: BucketId) -> Result<Self, ClientError> {
        Self::new(ws_url, EventFilter::bucket(bucket_id)).await
    }
}

impl Stream for EventStream {
    type Item = StorageEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(rx) = &mut self.subscriber.event_rx {
            Pin::new(rx).poll_recv(cx)
        } else {
            Poll::Ready(None)
        }
    }
}

// ============================================================================
// Callback-based Subscription
// ============================================================================

/// Type alias for event callbacks.
pub type EventCallback = Box<dyn Fn(StorageEvent) + Send + Sync>;

/// Handle for controlling a callback-based subscription.
pub struct SubscriptionHandle {
    running: Arc<AtomicBool>,
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl SubscriptionHandle {
    /// Check if the subscription is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Stop the subscription.
    pub async fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.task_handle.take() {
            let _ = handle.await;
        }
    }
}

/// Subscribe to events with a callback function.
///
/// # Example
///
/// ```no_run
/// use storage_client::event_subscription::{subscribe_with_callback, EventFilter, StorageEvent};
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let handle = subscribe_with_callback(
///     "ws://localhost:2222",
///     EventFilter::checkpoints_only(),
///     Box::new(|event| {
///         if let StorageEvent::BucketCheckpointed { bucket_id, .. } = event {
///             println!("Checkpoint for bucket {}", bucket_id);
///         }
///     }),
/// ).await?;
///
/// // ... do other work ...
///
/// // Stop when done
/// // handle.stop().await;
/// # Ok(())
/// # }
/// ```
pub async fn subscribe_with_callback(
    ws_url: &str,
    filter: EventFilter,
    callback: EventCallback,
) -> Result<SubscriptionHandle, ClientError> {
    let mut subscriber = EventSubscriber::connect(ws_url).await?;
    subscriber.set_filter(filter);
    subscriber.start().await?;

    let running = subscriber.running.clone();
    let mut event_rx = subscriber.event_rx.take().unwrap();

    let handle = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            callback(event);
        }
    });

    Ok(SubscriptionHandle {
        running,
        task_handle: Some(handle),
    })
}

// ============================================================================
// Convenience Functions
// ============================================================================

/// Subscribe to checkpoint events only.
pub async fn subscribe_checkpoints(
    ws_url: &str,
    callback: EventCallback,
) -> Result<SubscriptionHandle, ClientError> {
    subscribe_with_callback(ws_url, EventFilter::checkpoints_only(), callback).await
}

/// Subscribe to challenge events only.
pub async fn subscribe_challenges(
    ws_url: &str,
    callback: EventCallback,
) -> Result<SubscriptionHandle, ClientError> {
    subscribe_with_callback(ws_url, EventFilter::challenges_only(), callback).await
}

/// Subscribe to events for a specific bucket.
pub async fn subscribe_bucket_events(
    ws_url: &str,
    bucket_id: BucketId,
    callback: EventCallback,
) -> Result<SubscriptionHandle, ClientError> {
    subscribe_with_callback(ws_url, EventFilter::bucket(bucket_id), callback).await
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_filter_all() {
        let filter = EventFilter::all();
        assert!(filter.include_checkpoints);
        assert!(filter.include_challenges);
        assert!(filter.include_agreements);
    }

    #[test]
    fn test_event_filter_bucket() {
        let filter = EventFilter::bucket(42);
        assert!(filter.bucket_ids.contains(&42));
        assert!(filter.include_checkpoints);
    }

    #[test]
    fn test_event_filter_checkpoints_only() {
        let filter = EventFilter::checkpoints_only();
        assert!(filter.include_checkpoints);
        assert!(!filter.include_challenges);
        assert!(!filter.include_agreements);
    }

    #[test]
    fn test_filter_matches_bucket() {
        let filter = EventFilter::bucket(1);

        let event1 = StorageEvent::BucketCheckpointed {
            bucket_id: 1,
            mmr_root: H256::zero(),
            start_seq: 0,
            leaf_count: 0,
            providers: vec![],
            block_hash: H256::zero(),
            block_number: 0,
        };

        let event2 = StorageEvent::BucketCheckpointed {
            bucket_id: 2,
            mmr_root: H256::zero(),
            start_seq: 0,
            leaf_count: 0,
            providers: vec![],
            block_hash: H256::zero(),
            block_number: 0,
        };

        assert!(filter.matches(&event1));
        assert!(!filter.matches(&event2));
    }

    #[test]
    fn test_filter_matches_event_type() {
        let filter = EventFilter::checkpoints_only();

        let checkpoint_event = StorageEvent::BucketCheckpointed {
            bucket_id: 1,
            mmr_root: H256::zero(),
            start_seq: 0,
            leaf_count: 0,
            providers: vec![],
            block_hash: H256::zero(),
            block_number: 0,
        };

        let challenge_event = StorageEvent::ChallengeCreated {
            challenge_id: (0, 0),
            bucket_id: 1,
            provider: AccountId32::new([0u8; 32]),
            challenger: AccountId32::new([0u8; 32]),
            respond_by: 0,
            block_hash: H256::zero(),
            block_number: 0,
        };

        assert!(filter.matches(&checkpoint_event));
        assert!(!filter.matches(&challenge_event));
    }

    #[test]
    fn test_event_helpers() {
        let event = StorageEvent::BucketCheckpointed {
            bucket_id: 42,
            mmr_root: H256::repeat_byte(0xAB),
            start_seq: 100,
            leaf_count: 50,
            providers: vec![AccountId32::new([1u8; 32])],
            block_hash: H256::repeat_byte(0xCD),
            block_number: 12345,
        };

        assert_eq!(event.bucket_id(), Some(42));
        assert!(event.provider().is_some());
        assert_eq!(event.block_number(), 12345);
        assert!(event.is_checkpoint_event());
        assert!(!event.is_challenge_event());
    }
}
