// SPDX-License-Identifier: Apache-2.0

//! Bucket visibility and the challenger tier: `set_bucket_visibility`, the
//! private-bucket challenge gate, and the tier snapshotted at challenge
//! creation.

use super::challenge::{advance_snapshot_root, setup_with_snapshot};
use super::*;
use crate::ChallengeResponse;
use codec::Encode;
use sp_core::{Pair, H256};
use storage_primitives::{
    ChallengeId, ChunkLocation, Commitment, CommitmentPayload, ReplicaSyncRecord, Role, Visibility,
};

const CHUNK: ChunkLocation = ChunkLocation {
    leaf_index: 0,
    chunk_index: 0,
};

#[test]
fn set_bucket_visibility_is_admin_only_and_flips_both_ways() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        register_provider(2, 200);
        let bucket_id = setup_agreement(2, 1, 50, 200);
        assert_eq!(
            Buckets::<Test>::get(bucket_id).unwrap().visibility,
            Visibility::Public
        );

        assert_noop!(
            StorageProvider::set_bucket_visibility(
                RuntimeOrigin::signed(3),
                bucket_id,
                Visibility::Private
            ),
            Error::<Test>::NotBucketAdmin
        );
        assert_noop!(
            StorageProvider::set_bucket_visibility(
                RuntimeOrigin::signed(1),
                999,
                Visibility::Private
            ),
            Error::<Test>::BucketNotFound
        );

        assert_ok!(StorageProvider::set_bucket_visibility(
            RuntimeOrigin::signed(1),
            bucket_id,
            Visibility::Private
        ));
        assert_eq!(
            Buckets::<Test>::get(bucket_id).unwrap().visibility,
            Visibility::Private
        );
        System::assert_last_event(RuntimeEvent::StorageProvider(
            Event::BucketVisibilityChanged {
                bucket_id,
                visibility: Visibility::Private,
            },
        ));

        // Unconditionally reversible.
        assert_ok!(StorageProvider::set_bucket_visibility(
            RuntimeOrigin::signed(1),
            bucket_id,
            Visibility::Public
        ));
        assert_eq!(
            Buckets::<Test>::get(bucket_id).unwrap().visibility,
            Visibility::Public
        );
    });
}

#[test]
fn private_bucket_gates_primary_challenges() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);
        assert_ok!(StorageProvider::set_bucket_visibility(
            RuntimeOrigin::signed(1),
            bucket_id,
            Visibility::Private
        ));

        // A stranger cannot challenge a primary on a private bucket.
        assert_noop!(
            StorageProvider::challenge_checkpoint(RuntimeOrigin::signed(3), bucket_id, 2, CHUNK),
            Error::<Test>::NotAuthorizedForPrivateBucket
        );

        // The owner of the primary agreement (also admin here) passes.
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            CHUNK
        ));

        // A member of any role passes.
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            3,
            Role::Reader
        ));
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            CHUNK
        ));

        // Flipping back to Public reopens primary challenges to everyone.
        assert_ok!(StorageProvider::remove_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            3
        ));
        assert_ok!(StorageProvider::set_bucket_visibility(
            RuntimeOrigin::signed(1),
            bucket_id,
            Visibility::Public
        ));
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            CHUNK
        ));
    });
}

#[test]
fn private_bucket_gates_offchain_challenges() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);
        assert_ok!(StorageProvider::set_bucket_visibility(
            RuntimeOrigin::signed(1),
            bucket_id,
            Visibility::Private
        ));

        // A genuine provider-signed off-chain commitment: the gate must fire
        // even when everything else about the challenge is valid.
        let pair = provider_signer(2);
        let commitment = Commitment {
            mmr_root: H256::repeat_byte(0xAB),
            start_seq: 0,
            leaf_count: 10,
        };
        let payload = CommitmentPayload::new(bucket_id, commitment);
        let sig = sp_runtime::MultiSignature::Sr25519(pair.sign(&payload.encode()));

        assert_noop!(
            StorageProvider::challenge_offchain(
                RuntimeOrigin::signed(3),
                bucket_id,
                2,
                commitment,
                CHUNK,
                sig.clone()
            ),
            Error::<Test>::NotAuthorizedForPrivateBucket
        );

        // The same call from the agreement owner passes the gate.
        assert_ok!(StorageProvider::challenge_offchain(
            RuntimeOrigin::signed(1),
            bucket_id,
            2,
            commitment,
            CHUNK,
            sig
        ));
    });
}

/// Insert a replica agreement (owner 4, provider 5) with a synced record so
/// `challenge_replica` has an on-chain commitment to challenge.
fn add_synced_replica(bucket_id: u64, provider: u64, owner: u64) {
    register_provider(provider, 200);
    crate::StorageAgreements::<Test>::insert(
        bucket_id,
        provider,
        crate::StorageAgreement::<Test> {
            owner,
            max_bytes: 50,
            payment_locked: 0,
            price_per_byte: 0,
            expires_at: System::block_number() + 200,
            extensions_blocked: false,
            role: storage_primitives::ProviderRole::Replica {
                sync_balance: 0,
                sync_price: 0,
                min_sync_interval: 0,
                last_sync: Some(ReplicaSyncRecord {
                    commitment: storage_primitives::Commitment {
                        mmr_root: H256::repeat_byte(0xAB),
                        start_seq: 0,
                        leaf_count: 10,
                    },
                    block: System::block_number(),
                }),
            },
            started_at: System::block_number(),
        },
    );
}

#[test]
fn private_bucket_replica_challenges_stay_open_and_replica_owners_stay_gated() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);
        add_synced_replica(bucket_id, 5, 4);
        assert_ok!(StorageProvider::set_bucket_visibility(
            RuntimeOrigin::signed(1),
            bucket_id,
            Visibility::Private
        ));

        // A replica-agreement owner is NOT authorized for primary challenges
        // on a private bucket (deliberately excluded by the design).
        assert_noop!(
            StorageProvider::challenge_checkpoint(RuntimeOrigin::signed(4), bucket_id, 2, CHUNK),
            Error::<Test>::NotAuthorizedForPrivateBucket
        );

        // But the replica itself is challengeable by anyone, even a stranger —
        // the anti-censorship guarantee.
        assert_ok!(StorageProvider::challenge_replica(
            RuntimeOrigin::signed(3),
            bucket_id,
            5,
            CHUNK
        ));
        // The stranger's challenge is snapshotted as public tier.
        let (_, _, ch) = Challenges::<Test>::iter().next().expect("challenge exists");
        assert!(!ch.authorized);
    });
}

#[test]
fn agreement_owner_is_authorized_tier_without_membership() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);
        add_synced_replica(bucket_id, 5, 4);

        // Account 4 owns the replica agreement but is no member: authorized
        // tier for the fee split (any agreement owner counts).
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(4),
            bucket_id,
            2,
            CHUNK
        ));
        let (_, _, ch) = Challenges::<Test>::iter().next().expect("challenge exists");
        assert!(ch.authorized);
    });
}

#[test]
fn tier_is_snapshotted_at_creation() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        let challenger_balance_before = Balances::free_balance(3);
        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            CHUNK
        ));

        // Becoming a member after creation does not upgrade the open
        // challenge: settlement still uses the snapshotted public tier.
        assert_ok!(StorageProvider::set_member(
            RuntimeOrigin::signed(1),
            bucket_id,
            3,
            Role::Reader
        ));

        run_to_block(2);
        advance_snapshot_root(bucket_id);
        let stake_before = Providers::<Test>::get(2).unwrap().stake;
        assert_ok!(StorageProvider::respond_to_challenge(
            RuntimeOrigin::signed(2),
            ChallengeId {
                deadline: 101,
                index: 0
            },
            ChallengeResponse::Superseded,
        ));

        // Public settlement: the full deposit (100) reimburses the provider,
        // stake untouched, counted under the public tier.
        assert_eq!(Balances::free_balance(3), challenger_balance_before - 100);
        assert_eq!(Providers::<Test>::get(2).unwrap().stake, stake_before);
        let stats = Providers::<Test>::get(2).unwrap().stats;
        assert_eq!(stats.challenges_received_public, 1);
        assert_eq!(stats.challenges_received_authorized, 0);
    });
}

#[test]
fn timeout_slash_counts_failed_not_received() {
    new_test_ext().execute_with(|| {
        frame_system::Pallet::<Test>::set_block_number(1);
        let bucket_id = setup_with_snapshot(2, 1);

        assert_ok!(StorageProvider::challenge_checkpoint(
            RuntimeOrigin::signed(3),
            bucket_id,
            2,
            CHUNK
        ));

        // Let the challenge time out (deadline 101) and get swept.
        run_to_block(103);

        let stats = Providers::<Test>::get(2).unwrap().stats;
        assert_eq!(stats.challenges_failed, 1);
        assert_eq!(stats.challenges_received_authorized, 0);
        assert_eq!(stats.challenges_received_public, 0);
    });
}
