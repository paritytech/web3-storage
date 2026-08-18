// SPDX-License-Identifier: Apache-2.0

//! Test doubles shared by `membership`'s and `verify`'s test modules, so a
//! fixture used by both isn't defined twice in the same crate.

use crate::error::MembershipError;
use crate::membership::{Invalidation, Member, MembershipInvalidations, MembershipResolver};
use sp_core::crypto::AccountId32;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    async fn fetch_members(&self, _bucket_id: BucketId) -> Result<Vec<Member>, MembershipError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == 1 {
            Ok(vec![(self.account.clone(), Role::Admin).into()])
        } else {
            Err(MembershipError::Unavailable("chain down".to_string()))
        }
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
