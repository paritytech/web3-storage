// SPDX-License-Identifier: GPL-3.0-only

//! Chain-backed [`MembershipResolver`].

use crate::chain_connection::{self, ChainWatch};
use provider_auth::{Member, MembershipError, MembershipResolver};
use sp_core::crypto::AccountId32;
use storage_primitives::{BucketId, Role};
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
        use subxt::dynamic::{At, Value};

        let api = self.api()?;

        let storage_query =
            subxt::dynamic::storage::<(Value,), Value>("StorageProvider", "Buckets");

        let at = api
            .at_current_block()
            .await
            .map_err(|e| MembershipError::Unavailable(e.to_string()))?;
        let result = at
            .storage()
            .try_fetch(storage_query, (Value::u128(bucket_id as u128),))
            .await
            .map_err(|e| MembershipError::Unavailable(e.to_string()))?;

        let bucket_value = match result {
            Some(v) => v,
            None => return Ok(vec![]),
        };

        let decoded = bucket_value.decode().map_err(|e| MembershipError::Decode {
            bucket_id,
            reason: e.to_string(),
        })?;

        let members_val = match decoded.at("members") {
            Some(v) => v,
            None => return Ok(vec![]),
        };

        // `members` is a `BoundedVec<Member>`. Depending on how the runtime's
        // scale-info nests it, scale_value may wrap the sequence (and each
        // `AccountId32`) in extra single-field composites, so walk the value
        // tree and pull out every `{ account, role }` struct rather than
        // assuming a fixed shape.
        let mut members = Vec::new();
        collect_members(members_val, &mut members);

        if members.is_empty() {
            tracing::warn!(bucket_id, value = ?members_val, "auth: decoded zero members");
        } else {
            tracing::debug!(bucket_id, count = members.len(), "auth: resolved members");
        }

        Ok(members)
    }
}

/// Recursively pull `(account, role)` pairs out of a decoded `members` value,
/// tolerating any wrapper composites that `BoundedVec` / `AccountId32` type
/// info introduces. A `Member` is the composite that carries both an
/// `account` and a `role` field.
fn collect_members<T>(val: &subxt::ext::scale_value::Value<T>, out: &mut Vec<Member>) {
    use subxt::dynamic::At;
    use subxt::ext::scale_value::{Composite, ValueDef};

    if let (Some(account_v), Some(role_v)) = (val.at("account"), val.at("role")) {
        if let Some(bytes) = extract_account_bytes(account_v) {
            out.push((AccountId32::from(bytes), extract_role(role_v)).into());
            return;
        }
    }

    match &val.value {
        ValueDef::Composite(Composite::Named(fields)) => {
            for field in fields {
                collect_members(&field.1, out);
            }
        }
        ValueDef::Composite(Composite::Unnamed(items)) => {
            for item in items {
                collect_members(item, out);
            }
        }
        _ => {}
    }
}

/// Extract a 32-byte account id, descending through any wrapper composites
/// (`AccountId32` -> `[u8; 32]` can be one or more composite layers) and
/// collecting the `u8` leaves.
fn extract_account_bytes<T>(val: &subxt::ext::scale_value::Value<T>) -> Option<[u8; 32]> {
    let mut bytes = Vec::with_capacity(32);
    collect_u8_leaves(val, &mut bytes);
    if bytes.len() == 32 {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Some(arr)
    } else {
        None
    }
}

fn collect_u8_leaves<T>(val: &subxt::ext::scale_value::Value<T>, out: &mut Vec<u8>) {
    use subxt::ext::scale_value::{Composite, Primitive, ValueDef};
    match &val.value {
        ValueDef::Primitive(Primitive::U128(n)) => out.push(*n as u8),
        ValueDef::Composite(Composite::Unnamed(items)) => {
            for item in items {
                collect_u8_leaves(item, out);
            }
        }
        ValueDef::Composite(Composite::Named(fields)) => {
            for field in fields {
                collect_u8_leaves(&field.1, out);
            }
        }
        _ => {}
    }
}

/// Decode a `Role`, descending through wrapper composites to the enum variant.
fn extract_role<T>(val: &subxt::ext::scale_value::Value<T>) -> Role {
    find_role_variant(val).unwrap_or(Role::Reader)
}

fn find_role_variant<T>(val: &subxt::ext::scale_value::Value<T>) -> Option<Role> {
    use subxt::ext::scale_value::{Composite, ValueDef};
    match &val.value {
        ValueDef::Variant(variant) => match variant.name.as_str() {
            "Admin" => Some(Role::Admin),
            "Writer" => Some(Role::Writer),
            "Reader" => Some(Role::Reader),
            _ => None,
        },
        ValueDef::Composite(Composite::Unnamed(items)) => {
            items.iter().find_map(|v| find_role_variant(v))
        }
        ValueDef::Composite(Composite::Named(fields)) => {
            fields.iter().find_map(|f| find_role_variant(&f.1))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Regression test for the dynamic decoding of `StorageProvider.Buckets`.
    #[test]
    fn collect_members_handles_chain_value_nesting() {
        use subxt::ext::scale_value::Value;

        let acct = [9u8; 32];
        // AccountId32 -> [u8; 32]: two composite layers around the byte leaves.
        let account = Value::unnamed_composite(vec![Value::unnamed_composite(
            acct.iter().map(|b| Value::u128(*b as u128)),
        )]);
        let member = Value::named_composite(vec![
            ("account".to_string(), account),
            ("role".to_string(), Value::unnamed_variant("Writer", vec![])),
        ]);
        let sequence = Value::unnamed_composite(vec![member]);
        // BoundedVec wrapper around the member sequence.
        let members_val = Value::unnamed_composite(vec![sequence]);

        let mut out = Vec::new();
        collect_members(&members_val, &mut out);

        assert_eq!(
            out.len(),
            1,
            "member must be recovered through both wrappers"
        );
        assert_eq!(out[0].account, AccountId32::from(acct));
        assert!(matches!(out[0].role, Role::Writer));
    }
}
