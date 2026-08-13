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

        // No such bucket: an empty member set, which the caller reads as "not a
        // member". Distinct from a bucket we cannot decode, below.
        let bucket_value = match result {
            Some(v) => v,
            None => return Ok(vec![]),
        };

        let decoded = bucket_value.decode().map_err(|e| MembershipError::Decode {
            bucket_id,
            reason: e.to_string(),
        })?;

        let members_val = decoded
            .at("members")
            .ok_or_else(|| MembershipError::Decode {
                bucket_id,
                reason: "bucket has no `members` field".to_string(),
            })?;

        // `members` is a `BoundedVec<Member>`. Depending on how the runtime's
        // scale-info nests it, scale_value may wrap the sequence (and each
        // `AccountId32`) in extra single-field composites, so walk the value
        // tree and pull out every `{ account, role }` struct rather than
        // assuming a fixed shape.
        let mut members = Vec::new();
        collect_members(bucket_id, members_val, &mut members)?;

        // `create_bucket` seeds an admin and `remove_member` refuses to drop the
        // last one, so an existing bucket always has members. None decoded means
        // the value did not have the shape we walked, not an empty bucket.
        if members.is_empty() {
            tracing::warn!(bucket_id, value = ?members_val, "auth: decoded zero members");
            return Err(MembershipError::Decode {
                bucket_id,
                reason: "no members found in the bucket's member set".to_string(),
            });
        }

        tracing::debug!(bucket_id, count = members.len(), "auth: resolved members");
        Ok(members)
    }
}

/// Recursively pull `(account, role)` pairs out of a decoded `members` value,
/// tolerating any wrapper composites that `BoundedVec` / `AccountId32` type
/// info introduces. A `Member` is the composite that carries both an
/// `account` and a `role` field.
fn collect_members<T>(
    bucket_id: BucketId,
    val: &subxt::ext::scale_value::Value<T>,
    out: &mut Vec<Member>,
) -> Result<(), MembershipError> {
    use subxt::dynamic::At;
    use subxt::ext::scale_value::{Composite, ValueDef};

    if let (Some(account_v), Some(role_v)) = (val.at("account"), val.at("role")) {
        if let Some(bytes) = extract_account_bytes(account_v) {
            // Defaulting an unreadable role would silently grant access; a shape
            // we cannot read is a decode failure, not a role.
            let role = find_role_variant(role_v).ok_or_else(|| MembershipError::Decode {
                bucket_id,
                reason: "unrecognised Role variant".to_string(),
            })?;
            out.push((AccountId32::from(bytes), role).into());
            return Ok(());
        }
    }

    match &val.value {
        ValueDef::Composite(Composite::Named(fields)) => {
            for field in fields {
                collect_members(bucket_id, &field.1, out)?;
            }
        }
        ValueDef::Composite(Composite::Unnamed(items)) => {
            for item in items {
                collect_members(bucket_id, item, out)?;
            }
        }
        _ => {}
    }
    Ok(())
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
        collect_members(1, &members_val, &mut out).expect("well-formed value");

        assert_eq!(
            out.len(),
            1,
            "member must be recovered through both wrappers"
        );
        assert_eq!(out[0].account, AccountId32::from(acct));
        assert!(matches!(out[0].role, Role::Writer));
    }

    #[test]
    fn an_unreadable_role_is_a_decode_error_not_a_reader() {
        use subxt::ext::scale_value::Value;

        let account = Value::unnamed_composite(vec![Value::unnamed_composite(
            [9u8; 32].iter().map(|b| Value::u128(*b as u128)),
        )]);
        let member = Value::named_composite(vec![
            ("account".to_string(), account),
            // A variant the provider does not know — a runtime reshape, not a role.
            (
                "role".to_string(),
                Value::unnamed_variant("Auditor", vec![]),
            ),
        ]);
        let members_val = Value::unnamed_composite(vec![Value::unnamed_composite(vec![member])]);

        let err = collect_members(7, &members_val, &mut Vec::new())
            .expect_err("unknown role must not decode");
        assert!(
            matches!(err, MembershipError::Decode { bucket_id: 7, .. }),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn an_unrecognised_shape_yields_no_members() {
        use subxt::ext::scale_value::Value;

        // Shape we do not recognise: no `{ account, role }` composite anywhere.
        let members_val = Value::unnamed_composite(vec![Value::u128(1), Value::u128(2)]);

        let mut out = Vec::new();
        collect_members(3, &members_val, &mut out).expect("the walk itself succeeds");
        // `fetch_members` turns this into `Decode`, since an existing bucket
        // always has at least one member.
        assert!(out.is_empty(), "nothing member-shaped to find");
    }
}
