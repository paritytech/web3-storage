// SPDX-License-Identifier: GPL-3.0-only

//! Chain-backed [`MembershipResolver`].

use crate::chain_connection::{self, ChainWatch};
use provider_auth::{Member, MembershipError, MembershipResolver};
use sp_core::crypto::AccountId32;
use storage_primitives::BucketId;
use storage_subxt::api::runtime_types::pallet_storage_provider::pallet::Member as RuntimeMember;
use subxt::{OnlineClient, PolkadotConfig};

/// Membership resolver over the node's shared chain connection, so lookups
/// follow reconnects instead of pinning their own socket.
pub struct ChainMembershipResolver {
    chain_rx: ChainWatch,
}

impl ChainMembershipResolver {
    pub fn new(chain_rx: ChainWatch) -> Self {
        Self { chain_rx }
    }

    /// Resolved per lookup so reconnects are picked up.
    fn api(&self) -> Result<OnlineClient<PolkadotConfig>, MembershipError> {
        chain_connection::current_api(&self.chain_rx)
            .map_err(|e| MembershipError::Unavailable(e.to_string()))
    }
}

#[async_trait::async_trait]
impl MembershipResolver for ChainMembershipResolver {
    async fn fetch_members(&self, bucket_id: BucketId) -> Result<Vec<Member>, MembershipError> {
        let api = self.api()?;

        // `unvalidated`: see the `storage-subxt` crate docs.
        let storage_address = storage_subxt::api::storage()
            .storage_provider()
            .buckets()
            .unvalidated();

        let at = api
            .at_current_block()
            .await
            .map_err(|e| MembershipError::Unavailable(e.to_string()))?;
        let result = at
            .storage()
            .try_fetch(storage_address, (bucket_id,))
            .await
            .map_err(|e| MembershipError::Unavailable(e.to_string()))?;

        // No such bucket: an empty member set, which the caller reads as "not a
        // member". Distinct from a bucket we cannot decode, below.
        let bucket_value = match result {
            Some(v) => v,
            None => return Ok(vec![]),
        };

        let bucket = bucket_value.decode().map_err(|e| MembershipError::Decode {
            bucket_id,
            reason: e.to_string(),
        })?;

        let members = member_roles(bucket.members.0);

        // `create_bucket` seeds an admin and `remove_member` refuses to drop the
        // last one, so zero members means something changed chain-side. The
        // caller reads it as "not a member".
        if members.is_empty() {
            tracing::warn!(bucket_id, "auth: bucket decoded with zero members");
        } else {
            tracing::debug!(bucket_id, count = members.len(), "auth: resolved members");
        }

        Ok(members)
    }
}

fn member_roles(members: Vec<RuntimeMember>) -> Vec<Member> {
    members
        .into_iter()
        .map(|m| (AccountId32::new(m.account.0), m.role.into()).into())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_primitives::Role;

    #[tokio::test]
    async fn chain_resolver_fails_cleanly_before_first_connect() {
        // Before the chain-state coordinator publishes a connection, auth
        // lookups must surface a retryable error rather than panic or hang.
        let (_tx, rx) = tokio::sync::watch::channel(None);
        let resolver = ChainMembershipResolver::new(rx);
        let err = resolver
            .fetch_members(1)
            .await
            .expect_err("no connection published yet");
        // Retryable, not a decode bug — the node maps this onto a 503.
        assert!(
            matches!(err, MembershipError::Unavailable(_)),
            "unexpected error: {err}"
        );
    }

    /// Pins the generated-type -> primitives conversion for every role.
    #[test]
    fn member_roles_converts_accounts_and_roles() {
        use storage_subxt::api::runtime_types::storage_primitives::Role as RuntimeRole;

        let member = |byte: u8, role: RuntimeRole| RuntimeMember {
            account: subxt::utils::AccountId32([byte; 32]),
            role,
        };

        let expected: Vec<Member> = vec![
            (AccountId32::new([1u8; 32]), Role::Admin).into(),
            (AccountId32::new([2u8; 32]), Role::Writer).into(),
            (AccountId32::new([3u8; 32]), Role::Reader).into(),
        ];
        assert_eq!(
            member_roles(vec![
                member(1, RuntimeRole::Admin),
                member(2, RuntimeRole::Writer),
                member(3, RuntimeRole::Reader),
            ]),
            expected
        );
    }
}
