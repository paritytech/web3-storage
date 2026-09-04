// SPDX-License-Identifier: Apache-2.0

//! Test doubles for this crate's test modules, kept in one place so the
//! resolver fixtures sit together - and so one used by both `membership` and
//! `verify` isn't defined twice.

use crate::error::MembershipError;
use crate::membership::{BucketAccess, Invalidation, MembershipInvalidations, MembershipResolver};
use sp_core::crypto::AccountId32;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use storage_primitives::{BucketId, Role};

/// Resolver that succeeds once, then fails every call after - so a test can
/// seed a fresh cache entry and then force the error arm on the very next
/// lookup.
pub(crate) struct FlakyResolver {
    pub(crate) account: AccountId32,
    pub(crate) calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl MembershipResolver for FlakyResolver {
    async fn fetch_access(&self, _bucket_id: BucketId) -> Result<BucketAccess, MembershipError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == 1 {
            Ok(BucketAccess::private(vec![(
                self.account.clone(),
                Role::Admin,
            )
                .into()]))
        } else {
            Err(MembershipError::Unavailable("chain down".to_string()))
        }
    }
}

/// Resolver that grants `account` an `Admin` role on every bucket and counts
/// how many times it was actually called, so a test can tell a cache hit from
/// a re-resolve.
pub(crate) struct CountingResolver {
    pub(crate) account: AccountId32,
    pub(crate) calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl MembershipResolver for CountingResolver {
    async fn fetch_access(&self, _bucket_id: BucketId) -> Result<BucketAccess, MembershipError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(BucketAccess::private(vec![(
            self.account.clone(),
            Role::Admin,
        )
            .into()]))
    }
}

/// Holds `inner`'s answer back until `proceed` flips, so a test can land an
/// invalidation while a fetch is in flight.
///
/// `inner` runs to completion first: its call counter ticks and its result is
/// decided before the gate blocks. That is what lets a test spin on the
/// counter to learn a fetch has started, and it keeps a counter-driven inner
/// resolver such as [`FlakyResolver`] on the same call numbering it would see
/// ungated. The gate holds the first call too, so a test that seeds a cache
/// entry before racing one starts with `proceed` open and closes it after.
pub(crate) struct Gated<R> {
    pub(crate) inner: R,
    pub(crate) proceed: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl<R: MembershipResolver> MembershipResolver for Gated<R> {
    async fn fetch_access(&self, bucket_id: BucketId) -> Result<BucketAccess, MembershipError> {
        let result = self.inner.fetch_access(bucket_id).await;
        while !self.proceed.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        result
    }
}

/// A [`MembershipInvalidations`] a test can load with one value to return on
/// the next [`drain`](MembershipInvalidations::drain), simulating an event
/// arriving between two lookups.
#[derive(Clone)]
pub(crate) struct QueuedInvalidations(Arc<Mutex<Option<Invalidation>>>);

impl QueuedInvalidations {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }

    pub(crate) fn queue(&self, invalidation: Invalidation) {
        *self.0.lock().unwrap() = Some(invalidation);
    }
}

impl MembershipInvalidations for QueuedInvalidations {
    fn drain(&self) -> Invalidation {
        self.0.lock().unwrap().take().unwrap_or(Invalidation::None)
    }
}
