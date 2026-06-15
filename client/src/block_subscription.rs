//! Pluggable finalized-block event subscriber.
//!
//! Provides [`BlockSubscriber`], which subscribes to finalized blocks and
//! dispatches each block's events through a registry of [`EventPlugin`]s.
//!
//! # Core types
//!
//! - [`EventPlugin`] — object-safe trait; implement once per concern (pallet).
//! - [`ParserPlugin`] — adapter pairing an [`EventParser`] with a `Fn(Vec<E>)` handler.
//! - [`BlockSubscriber`] — registry + subscription loop (see its own docs).

use crate::event_subscription::{EventParser, StorageEvent, StorageProviderEventParser};
use crate::ClientError;
use sp_core::H256;
use std::marker::PhantomData;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use subxt::{events::Events, OnlineClient, PolkadotConfig};

// ============================================================================
// Error
// ============================================================================

/// Non-fatal error returned by a plugin's [`EventPlugin::process_block`].
///
/// The subscriber logs this and continues — it does not abort the loop.
#[derive(Debug, thiserror::Error)]
#[error("plugin '{name}' failed: {source}")]
pub struct PluginError {
    /// Name of the plugin that failed.
    pub name: String,
    #[source]
    pub source: Box<dyn std::error::Error + Send + Sync + 'static>,
}

// ============================================================================
// EventPlugin trait
// ============================================================================

/// Object-safe trait for finalized-block event plugins.
///
/// Each plugin handles one concern (typically one pallet). The subscriber calls
/// [`process_block`] for every finalized block, passing all events so the plugin
/// can filter and parse only what it understands.
///
/// Return [`Err`] for recoverable failures; the subscriber logs it and moves on
/// to the next plugin. Panics are caught by the subscriber via `catch_unwind`.
///
/// [`process_block`]: EventPlugin::process_block
pub trait EventPlugin: Send + Sync {
    /// Identifying name used in logs and error messages.
    fn name(&self) -> &str;

    /// Parse and handle all relevant events from a single finalized block.
    fn process_block(
        &self,
        events: &Events<PolkadotConfig>,
        block_hash: H256,
        block_number: u32,
    ) -> Result<(), PluginError>;
}

// ============================================================================
// ParserPlugin adapter
// ============================================================================

/// Adapter pairing an [`EventParser<E>`] with a per-plugin `Fn(Vec<E>)` handler.
///
/// The concrete event type `E` is closed over inside this struct; the subscriber
/// stores it as `Box<dyn EventPlugin>` without needing `Box<dyn Any>` or
/// downcasting.
///
/// `P` is a zero-sized marker type implementing [`EventParser<E>`]; it is never
/// instantiated — its associated functions are called statically. Pass it as a
/// type argument.
///
/// # Example
///
/// ```no_run
/// use storage_client::block_subscription::ParserPlugin;
/// use storage_client::{StorageProviderEventParser, StorageEvent};
///
/// let plugin = ParserPlugin::<StorageEvent, StorageProviderEventParser, _>::new(
///     "storage-provider",
///     |events| {
///         for ev in events {
///             println!("{ev:?}");
///         }
///     },
/// );
/// ```
pub struct ParserPlugin<E, P, H>
where
    P: EventParser<E>,
    H: Fn(Vec<E>) + Send + Sync + 'static,
    E: Send + 'static,
{
    name: String,
    handler: H,
    // fn() -> (E, P) is always Send + Sync regardless of E/P variance, because
    // P is never instantiated and E is only produced (not consumed) by the parser.
    _marker: PhantomData<fn() -> (E, P)>,
}

impl<E, P, H> ParserPlugin<E, P, H>
where
    P: EventParser<E>,
    H: Fn(Vec<E>) + Send + Sync + 'static,
    E: Send + 'static,
{
    /// Create a plugin from parser type `P` and a handler closure.
    pub fn new(name: impl Into<String>, handler: H) -> Self {
        Self {
            name: name.into(),
            handler,
            _marker: PhantomData,
        }
    }
}

impl<E, P, H> EventPlugin for ParserPlugin<E, P, H>
where
    P: EventParser<E>,
    H: Fn(Vec<E>) + Send + Sync + 'static,
    E: Send + 'static,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn process_block(
        &self,
        events: &Events<PolkadotConfig>,
        block_hash: H256,
        block_number: u32,
    ) -> Result<(), PluginError> {
        let parsed: Vec<E> = events
            .iter()
            .filter_map(|r| {
                let event = r.ok()?;
                P::parse_event_detail(&event, block_hash, block_number)
            })
            .collect();
        if !parsed.is_empty() {
            (self.handler)(parsed);
        }
        Ok(())
    }
}

/// Build a plugin that decodes `StorageProvider` pallet events into
/// [`StorageEvent`]s and forwards each block's batch to `handler`.
///
/// Convenience wrapper over [`ParserPlugin::new`] for the most common case.
pub fn storage_provider_plugin<H>(
    handler: H,
) -> ParserPlugin<StorageEvent, StorageProviderEventParser, H>
where
    H: Fn(Vec<StorageEvent>) + Send + Sync + 'static,
{
    ParserPlugin::new("storage-provider", handler)
}

// ============================================================================
// BlockSubscriber
// ============================================================================

/// Subscribes to finalized blocks and dispatches every block's events through a
/// registry of [`EventPlugin`]s.
///
/// Each finalized block is fetched once; its full event set (all pallets) is
/// handed to every registered plugin. A plugin that returns an error or panics is
/// logged and skipped — it never aborts the loop or starves the other plugins.
///
/// # Example
///
/// ```no_run
/// use storage_client::block_subscription::{BlockSubscriber, ParserPlugin};
/// use storage_client::{StorageProviderEventParser, StorageEvent};
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let mut subscriber = BlockSubscriber::connect("ws://localhost:2222")
///     .await?
///     .register(ParserPlugin::<StorageEvent, StorageProviderEventParser, _>::new(
///         "storage-provider",
///         |events| {
///             for ev in events {
///                 println!("{ev:?}");
///             }
///         },
///     ));
///
/// subscriber.start().await?;
/// // ... run until done ...
/// subscriber.stop().await;
/// # Ok(())
/// # }
/// ```
pub struct BlockSubscriber {
    api: OnlineClient<PolkadotConfig>,
    plugins: Arc<Vec<Box<dyn EventPlugin>>>,
    running: Arc<AtomicBool>,
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl BlockSubscriber {
    /// Connect to a node by WebSocket URL.
    pub async fn connect(ws_url: &str) -> Result<Self, ClientError> {
        let api = OnlineClient::<PolkadotConfig>::from_url(ws_url)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to connect: {e}")))?;
        Ok(Self::from_client(api))
    }

    /// Build a subscriber on top of an existing subxt client.
    pub fn from_client(api: OnlineClient<PolkadotConfig>) -> Self {
        Self {
            api,
            plugins: Arc::new(Vec::new()),
            running: Arc::new(AtomicBool::new(false)),
            task_handle: None,
        }
    }

    /// Register a plugin. Builder-style; call before [`start`](Self::start).
    ///
    /// # Panics
    ///
    /// Panics if called while the subscription is running — registration is a
    /// setup-time operation.
    pub fn register(mut self, plugin: impl EventPlugin + 'static) -> Self {
        Arc::get_mut(&mut self.plugins)
            .expect("BlockSubscriber::register must be called before start()")
            .push(Box::new(plugin));
        self
    }

    /// Start the finalized-block subscription. Idempotent: a second call while
    /// already running is a no-op.
    pub async fn start(&mut self) -> Result<(), ClientError> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        let api = self.api.clone();
        let plugins = self.plugins.clone();
        let running = self.running.clone();

        let handle = tokio::spawn(async move {
            if let Err(e) = Self::run_loop(api, plugins, running.clone()).await {
                tracing::error!("block subscription loop error: {e}");
            }
            running.store(false, Ordering::SeqCst);
        });

        self.task_handle = Some(handle);
        Ok(())
    }

    /// Stop the subscription and wait for the background task to finish.
    pub async fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.task_handle.take() {
            let _ = handle.await;
        }
    }

    /// Whether the subscription loop is currently running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Number of registered plugins.
    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    async fn run_loop(
        api: OnlineClient<PolkadotConfig>,
        plugins: Arc<Vec<Box<dyn EventPlugin>>>,
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

                    match block.events().await {
                        Ok(events) => {
                            for plugin in plugins.iter() {
                                isolate(plugin.name(), || {
                                    plugin.process_block(&events, block_hash, block_number)
                                });
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to fetch events for block {block_number}: {e}");
                        }
                    }
                }
                Some(Err(e)) => {
                    tracing::error!("Block subscription error: {e}");
                }
                None => break,
            }
        }

        Ok(())
    }
}

// ============================================================================
// Per-plugin isolation
// ============================================================================

/// Result of running a single plugin under isolation.
#[derive(Debug, PartialEq, Eq)]
enum PluginOutcome {
    Ok,
    Failed,
    Panicked,
}

/// Run one plugin invocation, catching panics and logging errors so a misbehaving
/// plugin cannot abort the dispatch loop.
fn isolate<F>(name: &str, f: F) -> PluginOutcome
where
    F: FnOnce() -> Result<(), PluginError>,
{
    match std::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => PluginOutcome::Ok,
        Ok(Err(e)) => {
            tracing::warn!("plugin '{name}' returned an error: {e}");
            PluginOutcome::Failed
        }
        Err(_) => {
            tracing::error!("plugin '{name}' panicked while processing a block");
            PluginOutcome::Panicked
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn _assert_send_sync<T: Send + Sync + 'static>() {}

    // A hand-written plugin that records block numbers without needing Events<C>.
    struct RecordingPlugin {
        name: String,
        seen: Arc<Mutex<Vec<u32>>>,
    }

    impl EventPlugin for RecordingPlugin {
        fn name(&self) -> &str {
            &self.name
        }

        fn process_block(
            &self,
            _events: &Events<PolkadotConfig>,
            _block_hash: H256,
            block_number: u32,
        ) -> Result<(), PluginError> {
            self.seen.lock().unwrap().push(block_number);
            Ok(())
        }
    }

    #[test]
    fn event_plugin_is_object_safe_and_send_sync() {
        // Constructing Box<dyn EventPlugin> verifies object-safety at compile time.
        let seen = Arc::new(Mutex::new(vec![]));
        let plugin: Box<dyn EventPlugin> = Box::new(RecordingPlugin {
            name: "recorder".to_string(),
            seen,
        });
        assert_eq!(plugin.name(), "recorder");
        _assert_send_sync::<Box<dyn EventPlugin>>();
    }

    #[test]
    fn parser_plugin_boxed_as_event_plugin() {
        struct NullParser;
        impl EventParser<u32> for NullParser {
            fn parse_event_detail(
                _event: &subxt::events::EventDetails<PolkadotConfig>,
                _block_hash: H256,
                _block_number: u32,
            ) -> Option<u32> {
                None
            }
        }

        let plugin: Box<dyn EventPlugin> = Box::new(ParserPlugin::<u32, NullParser, _>::new(
            "null-parser",
            |_| {},
        ));
        assert_eq!(plugin.name(), "null-parser");
    }

    #[test]
    fn parser_plugin_handler_receives_parsed_events() {
        // Tests the handler-dispatch path directly (bypasses Events<PolkadotConfig>
        // construction, which requires a live chain). The parsing step is exercised
        // end-to-end in the subscriber integration tests (t2).
        struct FakeParser;
        impl EventParser<u32> for FakeParser {
            fn parse_event_detail(
                _event: &subxt::events::EventDetails<PolkadotConfig>,
                _block_hash: H256,
                _block_number: u32,
            ) -> Option<u32> {
                Some(42)
            }
        }

        let received: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(vec![]));
        let r = received.clone();
        let plugin = ParserPlugin::<u32, FakeParser, _>::new("fake", move |evs| {
            r.lock().unwrap().extend(evs);
        });

        // Directly invoke the handler with a synthetic Vec — tests that the closure
        // receives exactly the events passed to it.
        (plugin.handler)(vec![42u32, 42u32]);

        assert_eq!(*received.lock().unwrap(), vec![42u32, 42u32]);
    }

    #[test]
    fn isolate_classifies_outcomes() {
        assert_eq!(isolate("ok", || Ok(())), PluginOutcome::Ok);

        let failed = isolate("err", || {
            Err(PluginError {
                name: "err".to_string(),
                source: "boom".into(),
            })
        });
        assert_eq!(failed, PluginOutcome::Failed);
    }

    #[test]
    fn isolation_lets_remaining_plugins_run_after_a_panic() {
        // Drives the dispatch path directly (no live chain). A panicking invocation
        // sandwiched between two good ones must not stop the good ones from running.
        use std::sync::atomic::AtomicUsize;

        let ran = Arc::new(AtomicUsize::new(0));
        let make = |should_panic: bool| {
            let ran = ran.clone();
            move || -> Result<(), PluginError> {
                if should_panic {
                    panic!("intentional plugin panic");
                }
                ran.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
        };

        // Silence the panic backtrace this test deliberately triggers.
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let o1 = isolate("good-1", make(false));
        let o2 = isolate("bad", make(true));
        let o3 = isolate("good-2", make(false));
        std::panic::set_hook(prev_hook);

        assert_eq!(o1, PluginOutcome::Ok);
        assert_eq!(o2, PluginOutcome::Panicked);
        assert_eq!(o3, PluginOutcome::Ok);
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
