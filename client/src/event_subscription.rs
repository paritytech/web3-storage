// SPDX-License-Identifier: Apache-2.0

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

use crate::scale_decode;
use crate::substrate::PALLET_NAME;
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
use subxt::ext::scale_value::{self, At};
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

    /// A provider updated its on-chain settings (pricing, capacity, availability).
    ProviderSettingsUpdated {
        provider: AccountId32,
        block_hash: H256,
        block_number: u32,
        provider_settings: crate::ProviderSettings,
    },

    /// A provider updated its on-chain multiaddr.
    ProviderMultiaddrUpdated {
        provider: AccountId32,
        multiaddr: String,
        block_hash: H256,
        block_number: u32,
    },

    // ========================================================================
    // Agreement Events
    // ========================================================================
    /// A primary storage agreement was established (provider-signed terms redeemed).
    StorageAgreementEstablished {
        bucket_id: BucketId,
        provider: AccountId32,
        owner: AccountId32,
        max_bytes: u64,
        duration: u32,
        price_per_byte: u128,
        expires_at: u32,
        block_hash: H256,
        block_number: u32,
    },

    /// A replica agreement was established (provider-signed replica terms redeemed).
    ReplicaAgreementEstablished {
        bucket_id: BucketId,
        provider: AccountId32,
        owner: AccountId32,
        max_bytes: u64,
        duration: u32,
        price_per_byte: u128,
        expires_at: u32,
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
            StorageEvent::StorageAgreementEstablished { bucket_id, .. } => Some(*bucket_id),
            StorageEvent::ReplicaAgreementEstablished { bucket_id, .. } => Some(*bucket_id),
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
            StorageEvent::ProviderSettingsUpdated { provider, .. } => Some(provider),
            StorageEvent::ProviderMultiaddrUpdated { provider, .. } => Some(provider),
            StorageEvent::StorageAgreementEstablished { provider, .. } => Some(provider),
            StorageEvent::ReplicaAgreementEstablished { provider, .. } => Some(provider),
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
            StorageEvent::ProviderSettingsUpdated { block_hash, .. } => *block_hash,
            StorageEvent::ProviderMultiaddrUpdated { block_hash, .. } => *block_hash,
            StorageEvent::StorageAgreementEstablished { block_hash, .. } => *block_hash,
            StorageEvent::ReplicaAgreementEstablished { block_hash, .. } => *block_hash,
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
            StorageEvent::ProviderSettingsUpdated { block_number, .. } => *block_number,
            StorageEvent::ProviderMultiaddrUpdated { block_number, .. } => *block_number,
            StorageEvent::StorageAgreementEstablished { block_number, .. } => *block_number,
            StorageEvent::ReplicaAgreementEstablished { block_number, .. } => *block_number,
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
            StorageEvent::StorageAgreementEstablished { .. }
                | StorageEvent::ReplicaAgreementEstablished { .. }
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
            StorageEvent::StorageAgreementEstablished { .. }
            | StorageEvent::ReplicaAgreementEstablished { .. }
            | StorageEvent::AgreementAccepted { .. }
            | StorageEvent::AgreementEnded { .. } => self.include_agreements,
            StorageEvent::BucketCreated { .. }
            | StorageEvent::BucketFrozen { .. }
            | StorageEvent::BucketDeleted { .. } => self.include_bucket_lifecycle,
            StorageEvent::ProviderRegistered { .. }
            | StorageEvent::ProviderAddedToBucket { .. }
            | StorageEvent::PrimaryProviderRemoved { .. }
            | StorageEvent::ProviderSettingsUpdated { .. }
            | StorageEvent::ProviderMultiaddrUpdated { .. } => self.include_provider_events,
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
                                        if event.pallet_name() == PALLET_NAME {
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

    /// Parse a subxt event detail into a [`StorageEvent`].
    ///
    /// Delegates to [`StorageProviderEventParser`] so that the subscription
    /// loop and one-shot callers share identical decoding logic.
    fn parse_event(
        event: &subxt::events::EventDetails<PolkadotConfig>,
        block_hash: H256,
        block_number: u32,
    ) -> Option<StorageEvent> {
        StorageProviderEventParser::parse_event_detail(event, block_hash, block_number)
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
// Event Parser
// ============================================================================

/// Trait for converting raw subxt events into a typed event enum.
///
/// Implement this for each pallet whose events you want to decode. The only
/// required method is [`parse_event_detail`]; [`from_extrinsic_events`] is
/// provided automatically and calls it for every event in the collection.
///
/// [`parse_event_detail`]: EventParser::parse_event_detail
/// [`from_extrinsic_events`]: EventParser::from_extrinsic_events
pub trait EventParser<EventType> {
    /// Attempt to decode a single [`subxt::events::EventDetails`] into an
    /// `EventType`. Return `None` to skip the event (wrong pallet, unknown
    /// variant, decode failure, etc.).
    fn parse_event_detail(
        event: &subxt::events::EventDetails<PolkadotConfig>,
        block_hash: H256,
        block_number: u32,
    ) -> Option<EventType>;

    /// Parse all events from a finalized extrinsic, returning only the ones
    /// that [`parse_event_detail`] accepts.
    ///
    /// The default implementation iterates the collection and calls
    /// [`parse_event_detail`] for each event, so implementors rarely need to
    /// override this.
    ///
    /// [`parse_event_detail`]: EventParser::parse_event_detail
    fn from_extrinsic_events(
        events: &subxt::blocks::ExtrinsicEvents<PolkadotConfig>,
        block_hash: H256,
        block_number: u32,
    ) -> Vec<EventType> {
        events
            .iter()
            .filter_map(|result| {
                let event = result.ok()?;
                Self::parse_event_detail(&event, block_hash, block_number)
            })
            .collect()
    }
}

/// Parser for converting raw subxt events into typed [`StorageEvent`]s.
///
/// `StorageProviderEventParser` is a stateless helper: every method is either associated
/// (takes no `self`) or free, so you can call it directly without constructing
/// an [`EventSubscriber`].
///
/// # Example — parse a finalized extrinsic's events
///
/// ```no_run
/// # use sp_core::H256;
/// # use storage_client::event_subscription::{EventParser, StorageProviderEventParser};
/// # use subxt::blocks::ExtrinsicEvents;
/// # use subxt::PolkadotConfig;
/// # async fn example(events: ExtrinsicEvents<PolkadotConfig>, block_hash: H256, block_number: u32) {
/// let storage_events =
///     StorageProviderEventParser::from_extrinsic_events(&events, block_hash, block_number);
/// for ev in storage_events {
///     println!("{ev:?}");
/// }
/// # }
/// ```
pub struct StorageProviderEventParser;

impl EventParser<StorageEvent> for StorageProviderEventParser {
    /// Parse a single [`subxt::events::EventDetails`] into a [`StorageEvent`].
    ///
    /// Returns `None` when the event:
    /// - comes from a pallet other than `StorageProvider`, or
    /// - has a variant that is not covered (e.g. `ProviderDeregistered`), or
    /// - cannot be decoded due to unexpected field structure.
    fn parse_event_detail(
        event: &subxt::events::EventDetails<PolkadotConfig>,
        block_hash: H256,
        block_number: u32,
    ) -> Option<StorageEvent> {
        if event.pallet_name() != PALLET_NAME {
            return None;
        }

        // We log decode failures at TRACE level so callers don't need to
        // worry about noisy warnings for known-unhandled variants.
        let fields = match event.field_values() {
            Ok(f) => f,
            Err(e) => {
                tracing::trace!("Failed to decode fields for {}: {e}", event.variant_name());
                return None;
            }
        };

        match event.variant_name() {
            // ── Checkpoint ────────────────────────────────────────────────────
            "BucketCheckpointed" => Some(StorageEvent::BucketCheckpointed {
                bucket_id: scale_decode::field_u64(&fields, "bucket_id")?,
                mmr_root: scale_decode::field_h256(&fields, "mmr_root")?,
                start_seq: scale_decode::field_u64(&fields, "start_seq")?,
                leaf_count: scale_decode::field_u64(&fields, "leaf_count")?,
                providers: scale_decode::field_accounts(&fields, "providers"),
                block_hash,
                block_number,
            }),

            // ── Challenges ────────────────────────────────────────────────────
            "ChallengeCreated" => {
                let (deadline, index) = field_challenge_id(&fields, "challenge_id")?;
                Some(StorageEvent::ChallengeCreated {
                    challenge_id: (deadline, index),
                    bucket_id: scale_decode::field_u64(&fields, "bucket_id")?,
                    provider: scale_decode::field_account(&fields, "provider")?,
                    challenger: scale_decode::field_account(&fields, "challenger")?,
                    respond_by: scale_decode::field_u32(&fields, "respond_by")?,
                    block_hash,
                    block_number,
                })
            }
            "ChallengeDefended" => {
                let (deadline, index) = field_challenge_id(&fields, "challenge_id")?;
                Some(StorageEvent::ChallengeDefended {
                    challenge_id: (deadline, index),
                    provider: scale_decode::field_account(&fields, "provider")?,
                    response_time_blocks: scale_decode::field_u32(&fields, "response_time_blocks")?,
                    challenger_cost: scale_decode::field_u128(&fields, "challenger_cost")?,
                    provider_cost: scale_decode::field_u128(&fields, "provider_cost")?,
                    block_hash,
                    block_number,
                })
            }
            "ChallengeSlashed" => {
                let (deadline, index) = field_challenge_id(&fields, "challenge_id")?;
                Some(StorageEvent::ChallengeSlashed {
                    challenge_id: (deadline, index),
                    provider: scale_decode::field_account(&fields, "provider")?,
                    slashed_amount: scale_decode::field_u128(&fields, "slashed_amount")?,
                    challenger_reward: scale_decode::field_u128(&fields, "challenger_reward")?,
                    block_hash,
                    block_number,
                })
            }

            // ── Providers ─────────────────────────────────────────────────────
            "ProviderRegistered" => Some(StorageEvent::ProviderRegistered {
                provider: scale_decode::field_account(&fields, "provider")?,
                stake: scale_decode::field_u128(&fields, "stake")?,
                block_hash,
                block_number,
            }),
            "ProviderAddedToBucket" => Some(StorageEvent::ProviderAddedToBucket {
                bucket_id: scale_decode::field_u64(&fields, "bucket_id")?,
                provider: scale_decode::field_account(&fields, "provider")?,
                block_hash,
                block_number,
            }),
            "PrimaryProviderRemoved" => Some(StorageEvent::PrimaryProviderRemoved {
                bucket_id: scale_decode::field_u64(&fields, "bucket_id")?,
                provider: scale_decode::field_account(&fields, "provider")?,
                reason: field_removal_reason(&fields, "reason"),
                block_hash,
                block_number,
            }),
            "ProviderSettingsUpdated" => Some(StorageEvent::ProviderSettingsUpdated {
                provider: scale_decode::field_account(&fields, "provider")?,
                provider_settings: field_provider_settings(&fields, "settings")?,
                block_hash,
                block_number,
            }),
            "ProviderMultiaddrUpdated" => Some(StorageEvent::ProviderMultiaddrUpdated {
                provider: scale_decode::field_account(&fields, "provider")?,
                multiaddr: String::from_utf8_lossy(&scale_decode::field_bytes(
                    &fields,
                    "multiaddr",
                )?)
                .into_owned(),
                block_hash,
                block_number,
            }),

            // ── Agreements ────────────────────────────────────────────────────
            "StorageAgreementEstablished" => {
                let (max_bytes, duration, price_per_byte) = field_terms_scalars(&fields, "terms");
                Some(StorageEvent::StorageAgreementEstablished {
                    bucket_id: scale_decode::field_u64(&fields, "bucket_id")?,
                    provider: scale_decode::field_account(&fields, "provider")?,
                    owner: scale_decode::field_account(&fields, "owner")?,
                    max_bytes,
                    duration,
                    price_per_byte,
                    expires_at: scale_decode::field_u32(&fields, "expires_at")?,
                    block_hash,
                    block_number,
                })
            }
            "ReplicaAgreementEstablished" => {
                let (max_bytes, duration, price_per_byte) = field_terms_scalars(&fields, "terms");
                Some(StorageEvent::ReplicaAgreementEstablished {
                    bucket_id: scale_decode::field_u64(&fields, "bucket_id")?,
                    provider: scale_decode::field_account(&fields, "provider")?,
                    owner: scale_decode::field_account(&fields, "owner")?,
                    max_bytes,
                    duration,
                    price_per_byte,
                    expires_at: scale_decode::field_u32(&fields, "expires_at")?,
                    block_hash,
                    block_number,
                })
            }
            "AgreementAccepted" => Some(StorageEvent::AgreementAccepted {
                bucket_id: scale_decode::field_u64(&fields, "bucket_id")?,
                provider: scale_decode::field_account(&fields, "provider")?,
                expires_at: scale_decode::field_u32(&fields, "expires_at")?,
                block_hash,
                block_number,
            }),
            "AgreementEnded" => Some(StorageEvent::AgreementEnded {
                bucket_id: scale_decode::field_u64(&fields, "bucket_id")?,
                provider: scale_decode::field_account(&fields, "provider")?,
                payment_to_provider: scale_decode::field_u128(&fields, "payment_to_provider")?,
                burned: scale_decode::field_u128(&fields, "burned")?,
                block_hash,
                block_number,
            }),

            // ── Buckets ───────────────────────────────────────────────────────
            "BucketCreated" => Some(StorageEvent::BucketCreated {
                bucket_id: scale_decode::field_u64(&fields, "bucket_id")?,
                admin: scale_decode::field_account(&fields, "admin")?,
                block_hash,
                block_number,
            }),
            "BucketFrozen" => Some(StorageEvent::BucketFrozen {
                bucket_id: scale_decode::field_u64(&fields, "bucket_id")?,
                frozen_start_seq: scale_decode::field_u64(&fields, "frozen_start_seq")?,
                block_hash,
                block_number,
            }),
            "BucketDeleted" => Some(StorageEvent::BucketDeleted {
                bucket_id: scale_decode::field_u64(&fields, "bucket_id")?,
                block_hash,
                block_number,
            }),

            // ── Replicas ──────────────────────────────────────────────────────
            "ReplicaSynced" => Some(StorageEvent::ReplicaSynced {
                bucket_id: scale_decode::field_u64(&fields, "bucket_id")?,
                provider: scale_decode::field_account(&fields, "provider")?,
                mmr_root: scale_decode::field_h256(&fields, "mmr_root")?,
                sync_payment: scale_decode::field_u128(&fields, "sync_payment")?,
                block_hash,
                block_number,
            }),

            // ── Everything else ───────────────────────────────────────────────
            other => Some(StorageEvent::Unknown {
                pallet: PALLET_NAME.to_string(),
                variant: other.to_string(),
                block_hash,
                block_number,
            }),
        }
    }
}

// Parser-specific field helpers — these encode shapes from the StorageProvider pallet
// (the `ChallengeId` struct and `RemovalReason` enum) and so stay alongside the parser
// rather than in [`crate::scale_decode`].

/// Read `max_bytes`, `duration`, and `price_per_byte` from a nested `AgreementTerms`
/// composite. Returns `(0, 0, 0)` if the composite is absent; individual scalars default
/// to 0 on decode failure so the outer event is never silently dropped.
fn field_terms_scalars(fields: &scale_value::Composite<u32>, name: &str) -> (u64, u32, u128) {
    let Some(terms) = fields.at(name) else {
        return (0, 0, 0);
    };
    let max_bytes = terms
        .at("max_bytes")
        .and_then(|v| v.as_u128())
        .map(|n| n as u64)
        .unwrap_or(0);
    let duration = terms
        .at("duration")
        .and_then(|v| v.as_u128())
        .map(|n| n as u32)
        .unwrap_or(0);
    let price_per_byte = terms
        .at("price_per_byte")
        .and_then(|v| v.as_u128())
        .unwrap_or(0);
    (max_bytes, duration, price_per_byte)
}

/// Read the StorageProvider pallet's `ChallengeId` named composite as a `(deadline, index)`
/// pair.
fn field_challenge_id(fields: &scale_value::Composite<u32>, name: &str) -> Option<(u32, u16)> {
    let v = fields.at(name)?;
    let deadline = v.at("deadline")?.as_u128()? as u32;
    let index = v.at("index")?.as_u128()? as u16;
    Some((deadline, index))
}

/// Read the StorageProvider pallet's `ProviderSettings` named composite into the client-side
/// [`crate::ProviderSettings`]. Returns `None` if a required field is missing or mistyped.
fn field_provider_settings(
    fields: &scale_value::Composite<u32>,
    name: &str,
) -> Option<crate::ProviderSettings> {
    let settings = fields.at(name)?;
    let replica_sync_price = match settings.at("replica_sync_price").map(|v| &v.value) {
        Some(scale_value::ValueDef::Variant(var)) if var.name == "Some" => {
            var.values.values().next().and_then(|v| v.as_u128())
        }
        _ => None,
    };
    Some(crate::ProviderSettings {
        price_per_byte: settings.at("price_per_byte")?.as_u128()?,
        min_duration: settings.at("min_duration")?.as_u128()? as u32,
        max_duration: settings.at("max_duration")?.as_u128()? as u32,
        accepting_primary: settings.at("accepting_primary")?.as_bool()?,
        replica_sync_price,
        accepting_extensions: settings.at("accepting_extensions")?.as_bool()?,
        max_capacity: settings.at("max_capacity")?.as_u128()? as u64,
    })
}

/// Read a `RemovalReason`-shaped variant field, falling back to `"Unknown"` when the field
/// is missing or not a variant.
fn field_removal_reason(fields: &scale_value::Composite<u32>, name: &str) -> String {
    fields
        .at(name)
        .and_then(scale_decode::variant_name)
        .unwrap_or_else(|| "Unknown".to_string())
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

    #[test]
    fn test_storage_agreement_established_helpers() {
        let provider = AccountId32::new([2u8; 32]);
        let owner = AccountId32::new([3u8; 32]);
        let event = StorageEvent::StorageAgreementEstablished {
            bucket_id: 7,
            provider: provider.clone(),
            owner,
            max_bytes: 1024,
            duration: 500,
            price_per_byte: 1_000_000,
            expires_at: 1000,
            block_hash: H256::repeat_byte(0x01),
            block_number: 999,
        };
        assert_eq!(event.bucket_id(), Some(7));
        assert_eq!(event.provider(), Some(&provider));
        assert_eq!(event.block_number(), 999);
        assert!(event.is_agreement_event());
        assert!(!event.is_checkpoint_event());
    }

    #[test]
    fn test_replica_agreement_established_helpers() {
        let provider = AccountId32::new([4u8; 32]);
        let owner = AccountId32::new([5u8; 32]);
        let event = StorageEvent::ReplicaAgreementEstablished {
            bucket_id: 8,
            provider: provider.clone(),
            owner,
            max_bytes: 2048,
            duration: 300,
            price_per_byte: 500_000,
            expires_at: 800,
            block_hash: H256::repeat_byte(0x02),
            block_number: 750,
        };
        assert_eq!(event.bucket_id(), Some(8));
        assert_eq!(event.provider(), Some(&provider));
        assert_eq!(event.block_number(), 750);
        assert!(event.is_agreement_event());
        assert!(!event.is_checkpoint_event());
    }
}
