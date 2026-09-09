// SPDX-License-Identifier: Apache-2.0

//! Benchmarking setup for pallet-storage-provider.
//!

use super::{Pallet as StorageProvider, *};
use frame_benchmarking::v2::*;
use frame_support::{
    pallet_prelude::*,
    traits::{Currency, Hooks, ReservableCurrency},
};
use frame_system::{Pallet as System, RawOrigin};
use sp_core::H256;
use sp_runtime::traits::{Bounded, SaturatedConversion};
use sp_runtime::Saturating;
use storage_primitives::{
    AgreementTerms, BucketId, ChunkLocation, Commitment, ProviderRole, ReplicaTerms,
};

const SEED: u32 = 0;

/// Advance the clock the pallet logic reads. `System` and the configured
/// [`Config::BlockNumberProvider`] are distinct clocks in a real runtime, so
/// benchmarks must advance both.
fn set_block_number<T: Config>(n: BlockNumberFor<T>) {
    use sp_runtime::traits::BlockNumberProvider;
    System::<T>::set_block_number(n);
    T::BlockNumberProvider::set_block_number(n);
}

/// Key type used by the benchmarking keystore for provider signing material.
const KEY_TYPE: sp_core::crypto::KeyTypeId = sp_core::crypto::KeyTypeId(*b"bnch");

fn funded_account<T: Config>(name: &'static str, index: u32) -> T::AccountId {
    let account: T::AccountId = account(name, index, SEED);
    let amount = BalanceOf::<T>::max_value() / 2u32.into();
    let _ = T::Currency::make_free_balance_be(&account, amount);
    account
}

fn create_provider<T: Config>(index: u32) -> T::AccountId {
    let provider = funded_account::<T>("provider", index);
    let multiaddr = b"/ip4/127.0.0.1/tcp/3000".to_vec();
    let public_key = [0u8; 32].to_vec();
    // Stake must cover max_capacity (1_000_000_000) * MinStakePerByte, not just MinProviderStake
    let capacity_stake: BalanceOf<T> = (1_000_000_000u64).saturated_into();
    let required_for_capacity = T::MinStakePerByte::get() * capacity_stake;
    let stake = T::MinProviderStake::get().max(required_for_capacity);

    let _ = Pallet::<T>::register_provider(
        RawOrigin::Signed(provider.clone()).into(),
        multiaddr.try_into().unwrap(),
        public_key.try_into().unwrap(),
        stake,
    );

    // Enable provider for agreements
    let _ = Pallet::<T>::update_provider_settings(
        RawOrigin::Signed(provider.clone()).into(),
        pallet::ProviderSettings {
            min_duration: 1u32.into(),
            max_duration: 1000u32.into(),
            price_per_byte: 1u32.into(),
            accepting_primary: true,
            replica_sync_price: Some(1000u32.into()),
            accepting_extensions: true,
            max_capacity: 1_000_000_000,
        },
    );

    provider
}

fn setup_bucket<T: Config>(admin: &T::AccountId) -> BucketId {
    // Use min_providers=0 so benchmarks can create checkpoints with empty signatures
    Pallet::<T>::create_bucket_internal(admin, 0, None).expect("create_bucket_internal succeeds")
}

/// Build primary [`AgreementTerms`] suitable for a benchmark agreement.
fn build_primary_terms<T: Config>(
    owner: &T::AccountId,
    max_bytes: u64,
    duration: BlockNumberFor<T>,
    nonce: u64,
) -> AgreementTermsOf<T> {
    AgreementTerms {
        owner: owner.clone(),
        max_bytes,
        duration,
        price_per_byte: 1u32.into(),
        valid_until: StorageProvider::<T>::current_anchor_block()
            .saturating_add(T::RequestTimeout::get()),
        nonce,
        bucket_id: None,
        replica_params: None,
    }
}

/// Build replica [`AgreementTerms`] suitable for a benchmark agreement.
fn build_replica_terms<T: Config>(
    owner: &T::AccountId,
    bucket_id: BucketId,
    max_bytes: u64,
    duration: BlockNumberFor<T>,
    nonce: u64,
) -> AgreementTermsOf<T> {
    AgreementTerms {
        owner: owner.clone(),
        max_bytes,
        duration,
        price_per_byte: 1u32.into(),
        valid_until: StorageProvider::<T>::current_anchor_block()
            .saturating_add(T::RequestTimeout::get()),
        nonce,
        bucket_id: Some(bucket_id),
        replica_params: Some(ReplicaTerms {
            sync_balance: BalanceOf::<T>::max_value() / 20u32.into(),
            min_sync_interval: 10u32.into(),
            sync_price: 10u32.into(),
        }),
    }
}

/// Sign SCALE-encoded terms with the provider's sr25519 keystore key.
fn sign_terms<T: Config>(
    public_key: &sp_core::sr25519::Public,
    terms: &AgreementTermsOf<T>,
) -> sp_runtime::MultiSignature {
    let hash = sp_io::hashing::blake2_256(&terms.signing_payload());
    let sig = sp_io::crypto::sr25519_sign(KEY_TYPE, public_key, &hash)
        .expect("benchmarking keystore signs with a key it generated");
    sp_runtime::MultiSignature::Sr25519(sig)
}

/// Open a primary agreement for the provider, atomically creating the bucket.
fn setup_primary_agreement<T: Config>(
    admin: &T::AccountId,
    provider: &T::AccountId,
    provider_index: u32,
) -> BucketId {
    let key = register_sr25519_key::<T>(provider, KEY_TYPE, provider_index);
    let terms = build_primary_terms::<T>(
        admin,
        1_000_000u64,
        100u32.into(),
        provider_index as u64 + 1,
    );
    let sig = sign_terms::<T>(&key, &terms);
    Pallet::<T>::establish_storage_agreement_internal(admin, provider, terms, &sig)
        .expect("establish_storage_agreement_internal succeeds")
}

/// Open a replica agreement against an existing bucket.
///
/// Generates an sr25519 key for the replica provider, signs replica terms,
/// and calls `establish_replica_agreement_internal`.
fn setup_replica_agreement<T: Config>(
    admin: &T::AccountId,
    bucket_id: BucketId,
    replica: &T::AccountId,
    replica_index: u32,
) {
    let key = register_sr25519_key::<T>(replica, KEY_TYPE, replica_index);
    let terms = build_replica_terms::<T>(
        admin,
        bucket_id,
        1_000_000u64,
        100u32.into(),
        replica_index as u64 + 1,
    );
    let sig = sign_terms::<T>(&key, &terms);
    Pallet::<T>::establish_replica_agreement_internal(admin, bucket_id, replica, terms, &sig)
        .expect("establish_replica_agreement_internal succeeds");
}

/// Direct-storage helper: register `provider` as a primary on an existing
/// bucket without going through `establish_storage_agreement_internal`.
///
/// `establish_storage_agreement_internal` always creates a fresh
/// single-primary bucket, so it can't grow the primary set on an existing
/// bucket. The checkpoint benchmarks need *N* primaries on the *same*
/// bucket to exercise worst-case signature verification, so we synthesize
/// that shape directly.
fn add_primary_to_bucket<T: Config>(
    admin: &T::AccountId,
    provider: &T::AccountId,
    bucket_id: BucketId,
    max_bytes: u64,
) {
    let anchor_block = StorageProvider::<T>::current_anchor_block();
    let duration: BlockNumberFor<T> = 100u32.into();
    let expires_at = anchor_block.saturating_add(duration);

    Buckets::<T>::mutate(bucket_id, |maybe| {
        if let Some(b) = maybe {
            let _ = b.primary_providers.try_push(provider.clone());
        }
    });

    let agreement = StorageAgreement::<T> {
        owner: admin.clone(),
        max_bytes,
        payment_locked: 0u32.into(),
        price_per_byte: 1u32.into(),
        expires_at,
        extensions_blocked: false,
        role: ProviderRole::Primary,
        started_at: anchor_block,
    };
    StorageAgreements::<T>::insert(bucket_id, provider, agreement);

    Providers::<T>::mutate(provider, |maybe| {
        if let Some(p) = maybe {
            p.committed_bytes = p.committed_bytes.saturating_add(max_bytes);
            p.stats.agreements_total = p.stats.agreements_total.saturating_add(1);
        }
    });
}

/// Insert a single challenge at `deadline = 200` and return its `ChallengeId`.
///
/// Used by the three `respond_to_challenge_*` benchmarks to share challenge
/// setup so each can focus on the response-variant payload it exercises.
fn insert_challenge<T: Config>(
    bucket_id: BucketId,
    provider: &T::AccountId,
    challenger: &T::AccountId,
    mmr_root: H256,
) -> storage_primitives::ChallengeId<BlockNumberFor<T>> {
    let deadline: BlockNumberFor<T> = 200u32.into();
    let challenge = pallet::Challenge::<T> {
        bucket_id,
        provider: provider.clone(),
        challenger: challenger.clone(),
        mmr_root,
        start_seq: 0,
        target: ChunkLocation {
            leaf_index: 0,
            chunk_index: 0,
        },
        deposit: 100u32.into(),
    };
    Challenges::<T>::insert(deadline, 0u16, challenge);
    storage_primitives::ChallengeId { deadline, index: 0 }
}

/// Register an sr25519 keypair for a provider and return the public key.
///
/// Overwrites the provider's stored public_key so `verify_signature` uses the
/// sr25519 key instead of the zeroed placeholder set by `create_provider`.
fn register_sr25519_key<T: Config>(
    provider: &T::AccountId,
    key_type: sp_core::crypto::KeyTypeId,
    index: u32,
) -> sp_core::sr25519::Public {
    let seed = alloc::format!("//BenchProvider{index}");
    let public_key = sp_io::crypto::sr25519_generate(key_type, Some(seed.into_bytes()));
    Providers::<T>::mutate(provider, |maybe_p| {
        if let Some(p) = maybe_p {
            p.public_key = public_key.0.to_vec().try_into().unwrap();
        }
    });
    public_key
}

#[benchmarks]
mod benchmarks {
    use super::*;

    // ─────────────────────────────────────────────────────────────────────────
    // Provider Management
    // ─────────────────────────────────────────────────────────────────────────

    #[benchmark]
    fn register_provider() {
        let caller = funded_account::<T>("caller", 0);
        let multiaddr: BoundedVec<u8, T::MaxMultiaddrLength> =
            b"/ip4/127.0.0.1/tcp/3000".to_vec().try_into().unwrap();
        let public_key: BoundedVec<u8, ConstU32<64>> = [0u8; 32].to_vec().try_into().unwrap();
        let stake = T::MinProviderStake::get();

        #[extrinsic_call]
        register_provider(RawOrigin::Signed(caller), multiaddr, public_key, stake);
    }

    #[benchmark]
    fn deregister_provider() {
        let _provider = create_provider::<T>(0);
        // Need to remove all agreements first - just use a fresh provider with no agreements

        let provider2 = funded_account::<T>("provider2", 99);
        let multiaddr = b"/ip4/127.0.0.1/tcp/3001".to_vec();
        let public_key = [0u8; 32].to_vec();
        let stake = T::MinProviderStake::get();

        let _ = Pallet::<T>::register_provider(
            RawOrigin::Signed(provider2.clone()).into(),
            multiaddr.try_into().unwrap(),
            public_key.try_into().unwrap(),
            stake,
        );

        #[extrinsic_call]
        deregister_provider(RawOrigin::Signed(provider2));
    }

    #[benchmark]
    fn complete_deregister() {
        let provider = create_provider::<T>(0);

        // Announce step.
        let _ = Pallet::<T>::deregister_provider(RawOrigin::Signed(provider.clone()).into());

        // Advance past `DeregisterAnnouncementPeriod` so completion is allowed.
        let announce_block = StorageProvider::<T>::current_anchor_block();
        let complete_after = announce_block.saturating_add(T::DeregisterAnnouncementPeriod::get());
        set_block_number::<T>(complete_after);

        #[extrinsic_call]
        complete_deregister(RawOrigin::Signed(provider));
    }

    /// Single provider mutate: clears `deregister_at` and restores acceptance flags.
    #[benchmark]
    fn cancel_deregister() {
        let provider = create_provider::<T>(0);
        let _ = Pallet::<T>::deregister_provider(RawOrigin::Signed(provider.clone()).into());

        #[extrinsic_call]
        cancel_deregister(RawOrigin::Signed(provider));
    }

    #[benchmark]
    fn update_provider_settings() {
        let provider = create_provider::<T>(0);
        let settings = pallet::ProviderSettings {
            min_duration: 10u32.into(),
            max_duration: 10000u32.into(),
            price_per_byte: 100u32.into(),
            accepting_primary: true,
            replica_sync_price: Some(5000u32.into()),
            accepting_extensions: true,
            max_capacity: 1_000_000_000,
        };

        #[extrinsic_call]
        update_provider_settings(RawOrigin::Signed(provider), settings);
    }

    #[benchmark]
    fn add_stake() {
        let provider = create_provider::<T>(0);
        let amount = T::MinProviderStake::get();

        #[extrinsic_call]
        add_stake(RawOrigin::Signed(provider), amount);
    }

    /// Worst case: provider has an existing agreement (storage read + write path).
    #[benchmark]
    fn block_extensions() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_primary_agreement::<T>(&admin, &provider, 0);

        #[extrinsic_call]
        set_extensions_blocked(RawOrigin::Signed(provider), bucket_id, true);
    }

    /// Worst case: provider has a max-length multiaddr stored (max write size).
    #[benchmark]
    fn update_provider_multiaddr() {
        let provider = create_provider::<T>(0);
        let max_len = T::MaxMultiaddrLength::get() as usize;
        let multiaddr: BoundedVec<u8, T::MaxMultiaddrLength> =
            alloc::vec![b'x'; max_len].try_into().unwrap();

        #[extrinsic_call]
        update_provider_multiaddr(RawOrigin::Signed(provider), multiaddr);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Bucket Management
    // ─────────────────────────────────────────────────────────────────────────

    #[benchmark]
    fn set_bucket_min_providers() {
        let admin = funded_account::<T>("admin", 0);
        let bucket_id = setup_bucket::<T>(&admin);

        #[extrinsic_call]
        set_min_providers(RawOrigin::Signed(admin), bucket_id, 0);
    }

    #[benchmark]
    fn freeze_bucket() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_primary_agreement::<T>(&admin, &provider, 0);

        // `freeze_bucket` requires the `snapshot` to meet `min_providers`.
        // Drop the floor to 0 first so both subsequent calls succeed.
        let _ =
            Pallet::<T>::set_min_providers(RawOrigin::Signed(admin.clone()).into(), bucket_id, 0);

        // Need to create a checkpoint first
        let mmr_root = H256::repeat_byte(0xAB);
        let signatures: BoundedVec<
            (T::AccountId, sp_runtime::MultiSignature),
            T::MaxPrimaryProviders,
        > = BoundedVec::new();

        let _ = Pallet::<T>::checkpoint(
            RawOrigin::Signed(admin.clone()).into(),
            bucket_id,
            Commitment {
                mmr_root,
                start_seq: 0,
                leaf_count: 10,
            },
            0u64, // nonce
            signatures,
        );

        #[extrinsic_call]
        freeze_bucket(RawOrigin::Signed(admin), bucket_id);
    }

    #[benchmark]
    fn set_bucket_member() {
        let admin = funded_account::<T>("admin", 0);
        let new_member = funded_account::<T>("member", 1);
        let bucket_id = setup_bucket::<T>(&admin);

        #[extrinsic_call]
        set_member(
            RawOrigin::Signed(admin),
            bucket_id,
            new_member,
            storage_primitives::Role::Writer,
        );
    }

    #[benchmark]
    fn remove_bucket_member() {
        let admin = funded_account::<T>("admin", 0);
        let member = funded_account::<T>("member", 1);
        let bucket_id = setup_bucket::<T>(&admin);

        // Add member first
        let _ = Pallet::<T>::set_member(
            RawOrigin::Signed(admin.clone()).into(),
            bucket_id,
            member.clone(),
            storage_primitives::Role::Writer,
        );

        #[extrinsic_call]
        remove_member(RawOrigin::Signed(admin), bucket_id, member);
    }

    #[benchmark]
    fn remove_slashed() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_primary_agreement::<T>(&admin, &provider, 0);

        // Simulate slashing: set provider stake to zero
        Providers::<T>::mutate(&provider, |maybe_provider| {
            if let Some(p) = maybe_provider {
                p.stake = 0u32.into();
            }
        });

        #[extrinsic_call]
        remove_slashed(RawOrigin::Signed(admin), bucket_id, provider);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Agreement Management
    // ─────────────────────────────────────────────────────────────────────────

    /// Worst case: full signature verification + replay-window mutation +
    /// bucket creation + agreement insertion.
    #[benchmark]
    fn establish_storage_agreement() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);

        // Generate an sr25519 key for the provider and store it so
        // verify_terms_signature can resolve a valid signer.
        let key = register_sr25519_key::<T>(&provider, KEY_TYPE, 0);
        let terms = build_primary_terms::<T>(&admin, 1_000_000u64, 100u32.into(), 1);
        let signature = sign_terms::<T>(&key, &terms);

        #[extrinsic_call]
        establish_storage_agreement(RawOrigin::Signed(admin), provider, terms, signature);
    }

    /// Worst case: replica signature verification + replay-window
    /// mutation + agreement insertion on top of an existing bucket.
    #[benchmark]
    fn establish_replica_agreement() {
        let admin = funded_account::<T>("admin", 0);
        let primary = create_provider::<T>(0);
        let bucket_id = setup_primary_agreement::<T>(&admin, &primary, 0);

        let replica = create_provider::<T>(1);
        let key = register_sr25519_key::<T>(&replica, KEY_TYPE, 1);
        let terms = build_replica_terms::<T>(&admin, bucket_id, 1_000_000u64, 100u32.into(), 1);
        let signature = sign_terms::<T>(&key, &terms);

        #[extrinsic_call]
        establish_replica_agreement(
            RawOrigin::Signed(admin),
            bucket_id,
            replica,
            terms,
            signature,
        );
    }

    #[benchmark]
    fn top_up_agreement() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_primary_agreement::<T>(&admin, &provider, 0);

        let additional_bytes = 500_000u64;
        let max_payment = BalanceOf::<T>::max_value() / 10u32.into();

        #[extrinsic_call]
        top_up_agreement(
            RawOrigin::Signed(admin),
            bucket_id,
            provider,
            additional_bytes,
            max_payment,
        );
    }

    #[benchmark]
    fn extend_agreement() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_primary_agreement::<T>(&admin, &provider, 0);

        let additional_duration: BlockNumberFor<T> = 50u32.into();
        let max_payment = BalanceOf::<T>::max_value() / 10u32.into();

        #[extrinsic_call]
        extend_agreement(
            RawOrigin::Signed(admin),
            bucket_id,
            provider,
            additional_duration,
            max_payment,
        );
    }

    #[benchmark]
    fn end_agreement(a: Linear<0, 1>) {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_primary_agreement::<T>(&admin, &provider, 0);

        // a == 0 → Pay (single transfer to provider)
        // a == 1 → Burn with burn_percent in (0, 100) → both owner→provider
        //          and owner→treasury transfers run (worst case).
        let action = match a {
            0 => storage_primitives::EndAction::Pay,
            _ => storage_primitives::EndAction::Burn { burn_percent: 50 },
        };

        // Ensure the treasury account exists so the burn-path transfer
        // (KeepAlive) doesn't fail with "Account cannot exist with the
        // funds that would be given".
        let _ = T::Currency::make_free_balance_be(
            &T::Treasury::get(),
            BalanceOf::<T>::max_value() / 2u32.into(),
        );

        #[extrinsic_call]
        end_agreement(RawOrigin::Signed(admin), bucket_id, provider, action);
    }

    #[benchmark]
    fn claim_expired_agreement() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_primary_agreement::<T>(&admin, &provider, 0);

        // Advance block past agreement expiry + settlement timeout
        let agreement = StorageProvider::<T>::storage_agreements(bucket_id, &provider).unwrap();
        let anchor_block = StorageProvider::<T>::current_anchor_block();
        let target_block: BlockNumberFor<T> = agreement
            .expires_at
            .saturating_add(T::SettlementTimeout::get())
            .saturating_add(anchor_block)
            .saturating_add(1u32.into());
        set_block_number::<T>(target_block);

        #[extrinsic_call]
        claim_expired_agreement(RawOrigin::Signed(provider), bucket_id);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Checkpoints
    // ─────────────────────────────────────────────────────────────────────────

    /// Worst case: MaxPrimaryProviders all sign — every signature is verified.
    #[benchmark]
    fn checkpoint() {
        let admin = funded_account::<T>("admin", 0);
        let bucket_id = setup_bucket::<T>(&admin);
        let n = T::MaxPrimaryProviders::get();
        let mmr_root = H256::repeat_byte(0xAB);

        // Create n providers, each with an sr25519 keypair, and set up primary agreements.
        let mut provider_keys: alloc::vec::Vec<(T::AccountId, sp_core::sr25519::Public)> =
            alloc::vec::Vec::new();
        for i in 0..n {
            let provider = create_provider::<T>(i);
            let key = register_sr25519_key::<T>(&provider, KEY_TYPE, i);
            add_primary_to_bucket::<T>(&admin, &provider, bucket_id, 1_000_000);
            provider_keys.push((provider, key));
        }

        // Require all n providers to sign (worst case — maximum signature verifications).
        let _ =
            Pallet::<T>::set_min_providers(RawOrigin::Signed(admin.clone()).into(), bucket_id, n);

        let commitment = Commitment {
            mmr_root,
            start_seq: 0,
            leaf_count: 10,
        };
        let payload = storage_primitives::CommitmentPayload::new(bucket_id, commitment, 0u64);
        let encoded_payload = codec::Encode::encode(&payload);

        let mut signatures: BoundedVec<
            (T::AccountId, sp_runtime::MultiSignature),
            T::MaxPrimaryProviders,
        > = BoundedVec::new();
        for (provider, key) in provider_keys {
            let sig = sp_io::crypto::sr25519_sign(KEY_TYPE, &key, &encoded_payload)
                .expect("signing should work");
            let _ = signatures.try_push((provider, sp_runtime::MultiSignature::Sr25519(sig)));
        }

        #[extrinsic_call]
        checkpoint(
            RawOrigin::Signed(admin),
            bucket_id,
            commitment,
            0u64, // nonce
            signatures,
        );
    }

    /// Worst case: MaxPrimaryProviders new signatures are each verified and bits set.
    #[benchmark]
    fn extend_checkpoint() {
        let admin = funded_account::<T>("admin", 0);
        let bucket_id = setup_bucket::<T>(&admin);
        let n = T::MaxPrimaryProviders::get();
        let mmr_root = H256::repeat_byte(0xAB);

        // Create n providers with sr25519 keys and primary agreements.
        let mut provider_keys: alloc::vec::Vec<(T::AccountId, sp_core::sr25519::Public)> =
            alloc::vec::Vec::new();
        for i in 0..n {
            let provider = create_provider::<T>(i);
            let key = register_sr25519_key::<T>(&provider, KEY_TYPE, i);
            add_primary_to_bucket::<T>(&admin, &provider, bucket_id, 1_000_000);
            provider_keys.push((provider, key));
        }

        // Submit initial checkpoint with 0 signatures (min_providers=0).
        // The snapshot bitfield has all bits unset, so every extend_checkpoint
        // signature triggers a real verification + bit-set operation.
        let empty_sigs: BoundedVec<
            (T::AccountId, sp_runtime::MultiSignature),
            T::MaxPrimaryProviders,
        > = BoundedVec::new();
        let commitment = Commitment {
            mmr_root,
            start_seq: 0,
            leaf_count: 10,
        };
        let _ = Pallet::<T>::checkpoint(
            RawOrigin::Signed(admin.clone()).into(),
            bucket_id,
            commitment,
            0u64, // nonce
            empty_sigs,
        );

        let payload = storage_primitives::CommitmentPayload::new(bucket_id, commitment, 0u64);
        let encoded_payload = codec::Encode::encode(&payload);

        let mut additional_signatures: BoundedVec<
            (T::AccountId, sp_runtime::MultiSignature),
            T::MaxPrimaryProviders,
        > = BoundedVec::new();
        for (provider, key) in provider_keys {
            let sig = sp_io::crypto::sr25519_sign(KEY_TYPE, &key, &encoded_payload)
                .expect("signing should work");
            let _ = additional_signatures
                .try_push((provider, sp_runtime::MultiSignature::Sr25519(sig)));
        }

        #[extrinsic_call]
        extend_checkpoint(RawOrigin::Signed(admin), bucket_id, additional_signatures);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Challenge System
    // ─────────────────────────────────────────────────────────────────────────

    #[benchmark]
    fn challenge_checkpoint() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_primary_agreement::<T>(&admin, &provider, 0);

        // Drop min_providers to 0 so the bootstrapping checkpoint with
        // empty signatures is accepted (establish_storage_agreement_internal
        // anchors the bucket at min_providers=1).
        let _ =
            Pallet::<T>::set_min_providers(RawOrigin::Signed(admin.clone()).into(), bucket_id, 0);

        // Create checkpoint first
        let mmr_root = H256::repeat_byte(0xAB);
        let signatures: BoundedVec<
            (T::AccountId, sp_runtime::MultiSignature),
            T::MaxPrimaryProviders,
        > = BoundedVec::new();

        let _ = Pallet::<T>::checkpoint(
            RawOrigin::Signed(admin.clone()).into(),
            bucket_id,
            Commitment {
                mmr_root,
                start_seq: 0,
                leaf_count: 10,
            },
            0u64, // nonce
            signatures,
        );

        // Set provider's bit in snapshot bitfield (provider is at index 0)
        Buckets::<T>::mutate(bucket_id, |maybe_bucket| {
            if let Some(bucket) = maybe_bucket {
                if let Some(snapshot) = bucket.snapshot.as_mut() {
                    if snapshot.primary_signers.is_empty() {
                        snapshot.primary_signers.push(0);
                    }
                    snapshot.primary_signers[0] |= 1; // Set bit 0
                }
            }
        });

        #[extrinsic_call]
        challenge_checkpoint(
            RawOrigin::Signed(admin),
            bucket_id,
            provider,
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        );
    }

    #[benchmark]
    fn challenge_off_chain() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_primary_agreement::<T>(&admin, &provider, 0);

        // Generate sr25519 keypair via host functions (works in no_std benchmarks)
        let key_type = sp_core::crypto::KeyTypeId(*b"bnch");
        let public_key = sp_io::crypto::sr25519_generate(key_type, Some(b"//Benchmark".to_vec()));
        Providers::<T>::mutate(&provider, |maybe_provider| {
            if let Some(p) = maybe_provider {
                p.public_key = public_key.0.to_vec().try_into().unwrap();
            }
        });

        // Sign the commitment payload via host function
        let mmr_root = H256::repeat_byte(0xAB);
        let commitment = Commitment {
            mmr_root,
            start_seq: 0,
            leaf_count: 0,
        };
        let payload = storage_primitives::CommitmentPayload::new(bucket_id, commitment, 0u64);
        let encoded = codec::Encode::encode(&payload);
        let sig = sp_io::crypto::sr25519_sign(key_type, &public_key, &encoded)
            .expect("signing should work");
        let signature = sp_runtime::MultiSignature::Sr25519(sig);

        #[extrinsic_call]
        challenge_offchain(
            RawOrigin::Signed(admin),
            bucket_id,
            provider,
            commitment,
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
            0u64, // nonce
            signature,
        );
    }

    #[benchmark]
    fn challenge_replica() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let replica_provider = create_provider::<T>(1);
        let bucket_id = setup_primary_agreement::<T>(&admin, &provider, 0);

        // Drop min_providers to 0 so the bootstrapping checkpoint with
        // empty signatures is accepted.
        let _ =
            Pallet::<T>::set_min_providers(RawOrigin::Signed(admin.clone()).into(), bucket_id, 0);

        // Create checkpoint so bucket has a snapshot
        let mmr_root = H256::repeat_byte(0xAB);
        let signatures: BoundedVec<
            (T::AccountId, sp_runtime::MultiSignature),
            T::MaxPrimaryProviders,
        > = BoundedVec::new();
        let _ = Pallet::<T>::checkpoint(
            RawOrigin::Signed(admin.clone()).into(),
            bucket_id,
            Commitment {
                mmr_root,
                start_seq: 0,
                leaf_count: 10,
            },
            0u64, // nonce
            signatures,
        );

        // Open the replica agreement via the signed-terms helper.
        setup_replica_agreement::<T>(&admin, bucket_id, &replica_provider, 1);

        // Confirm replica sync so replica has a last_sync root
        let roots: [Option<H256>; 7] = [Some(mmr_root), None, None, None, None, None, None];
        let sig =
            sp_runtime::MultiSignature::Sr25519(sp_core::sr25519::Signature::from_raw([0u8; 64]));
        let _ = Pallet::<T>::confirm_replica_sync(
            RawOrigin::Signed(replica_provider.clone()).into(),
            bucket_id,
            roots,
            sig,
        );

        #[extrinsic_call]
        challenge_replica(
            RawOrigin::Signed(admin),
            bucket_id,
            replica_provider,
            ChunkLocation {
                leaf_index: 0,
                chunk_index: 0,
            },
        );
    }

    /// `Proof` response — hashes MaxChunkSize bytes and verifies MMR + Merkle proofs.
    #[benchmark]
    fn respond_to_challenge_proof() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_primary_agreement::<T>(&admin, &provider, 0);

        // Construct a single-chunk / single-leaf MMR so all proof verifications pass.
        //
        // Chunk tree (1 chunk):  data_root = blake2_256(chunk_data)
        //   chunk_proof = empty (leaf IS the root, no siblings)
        //
        // MMR (1 leaf):  mmr_root = blake2_256(encode(mmr_leaf))
        //   leaf_proof = empty (leaf IS the single peak, no siblings)
        //   peaks = [mmr_root]
        let chunk_size = T::MaxChunkSize::get() as usize;
        let chunk_bytes = alloc::vec![0xBEu8; chunk_size];
        let chunk_data: BoundedVec<u8, T::MaxChunkSize> =
            BoundedVec::try_from(chunk_bytes.clone()).unwrap();

        let chunk_hash = storage_primitives::blake2_256(&chunk_bytes);
        let data_root = chunk_hash; // single-element Merkle tree

        let mmr_leaf = storage_primitives::MmrLeaf {
            data_root,
            data_size: chunk_size as u64,
            total_size: chunk_size as u64,
        };
        let leaf_hash = storage_primitives::blake2_256(&codec::Encode::encode(&mmr_leaf));
        let mmr_root = leaf_hash; // single-peak MMR, root == peak == leaf_hash

        let mmr_proof = storage_primitives::MmrProof {
            peaks: alloc::vec![leaf_hash],
            leaf: mmr_leaf,
            leaf_proof: storage_primitives::MerkleProof {
                siblings: alloc::vec![],
                path: alloc::vec![],
            },
        };
        let chunk_proof = storage_primitives::MerkleProof {
            siblings: alloc::vec![],
            path: alloc::vec![],
        };

        let challenge_id = insert_challenge::<T>(bucket_id, &provider, &admin, mmr_root);

        let response: pallet::ChallengeResponse<T> = pallet::ChallengeResponse::Proof {
            chunk_data,
            mmr_proof,
            chunk_proof,
        };

        #[extrinsic_call]
        respond_to_challenge(RawOrigin::Signed(provider), challenge_id, response);
    }

    /// `Deleted` response — admin check + sr25519 signature verification on the deletion payload.
    #[benchmark]
    fn respond_to_challenge_deleted() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_primary_agreement::<T>(&admin, &provider, 0);

        // `verify_signature` looks up the signer (admin) in `Providers`, so admin
        // must have a provider record with a valid sr25519 public key.
        let _ = Pallet::<T>::register_provider(
            RawOrigin::Signed(admin.clone()).into(),
            b"/ip4/127.0.0.1/tcp/3001".to_vec().try_into().unwrap(),
            [0u8; 32].to_vec().try_into().unwrap(),
            T::MinProviderStake::get(),
        );
        let admin_key = register_sr25519_key::<T>(&admin, KEY_TYPE, u32::MAX);

        let challenge_id =
            insert_challenge::<T>(bucket_id, &provider, &admin, H256::repeat_byte(0xCD));

        let new_mmr_root = H256::repeat_byte(0xEF);
        let new_start_seq: u64 = 1; // must be > challenge.start_seq (0) + leaf_index (0)
        let payload = storage_primitives::CommitmentPayload::new(
            bucket_id,
            Commitment {
                mmr_root: new_mmr_root,
                start_seq: new_start_seq,
                leaf_count: 0,
            },
            0u64, /* nonce */
        );
        let encoded = codec::Encode::encode(&payload);
        let sig = sp_io::crypto::sr25519_sign(KEY_TYPE, &admin_key, &encoded)
            .expect("signing should work");
        let admin_signature = sp_runtime::MultiSignature::Sr25519(sig);

        let response: pallet::ChallengeResponse<T> = pallet::ChallengeResponse::Deleted {
            new_mmr_root,
            new_start_seq,
            nonce: 0u64,
            admin: admin.clone(),
            admin_signature,
        };

        #[extrinsic_call]
        respond_to_challenge(RawOrigin::Signed(provider), challenge_id, response);
    }

    /// `Superseded` response — bucket-snapshot lookup + sequence-number comparison (cheapest path).
    #[benchmark]
    fn respond_to_challenge_superseded() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_primary_agreement::<T>(&admin, &provider, 0);

        // Drop min_providers to 0 so the bootstrapping checkpoint below
        // with empty signatures is accepted.
        let _ =
            Pallet::<T>::set_min_providers(RawOrigin::Signed(admin.clone()).into(), bucket_id, 0);

        // `Superseded` requires the bucket to have a snapshot whose
        // (start_seq + leaf_count) exceeds the challenged sequence. A
        // checkpoint with leaf_count > 0 satisfies this for our challenge
        // at seq 0.
        let mmr_root = H256::repeat_byte(0xAB);
        let signatures: BoundedVec<
            (T::AccountId, sp_runtime::MultiSignature),
            T::MaxPrimaryProviders,
        > = BoundedVec::new();
        let _ = Pallet::<T>::checkpoint(
            RawOrigin::Signed(admin.clone()).into(),
            bucket_id,
            Commitment {
                mmr_root,
                start_seq: 0,
                leaf_count: 10,
            },
            0u64, // nonce
            signatures,
        );

        let challenge_id =
            insert_challenge::<T>(bucket_id, &provider, &admin, H256::repeat_byte(0xCD));

        let response: pallet::ChallengeResponse<T> = pallet::ChallengeResponse::Superseded;

        #[extrinsic_call]
        respond_to_challenge(RawOrigin::Signed(provider), challenge_id, response);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Replica Sync
    // ─────────────────────────────────────────────────────────────────────────

    #[benchmark]
    fn confirm_replica_sync() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let replica_provider = create_provider::<T>(1);
        let bucket_id = setup_primary_agreement::<T>(&admin, &provider, 0);

        // Drop min_providers to 0 so the bootstrapping checkpoint with
        // empty signatures is accepted.
        let _ =
            Pallet::<T>::set_min_providers(RawOrigin::Signed(admin.clone()).into(), bucket_id, 0);

        // Create checkpoint so bucket has a snapshot with known mmr_root
        let mmr_root = H256::repeat_byte(0xAB);
        let signatures: BoundedVec<
            (T::AccountId, sp_runtime::MultiSignature),
            T::MaxPrimaryProviders,
        > = BoundedVec::new();
        let _ = Pallet::<T>::checkpoint(
            RawOrigin::Signed(admin.clone()).into(),
            bucket_id,
            Commitment {
                mmr_root,
                start_seq: 0,
                leaf_count: 10,
            },
            0u64, // nonce
            signatures,
        );

        // Open the replica agreement via the signed-terms helper.
        setup_replica_agreement::<T>(&admin, bucket_id, &replica_provider, 1);

        // roots[0] matches current snapshot mmr_root
        let roots: [Option<H256>; 7] = [Some(mmr_root), None, None, None, None, None, None];
        let signature =
            sp_runtime::MultiSignature::Sr25519(sp_core::sr25519::Signature::from_raw([0u8; 64]));

        #[extrinsic_call]
        confirm_replica_sync(
            RawOrigin::Signed(replica_provider),
            bucket_id,
            roots,
            signature,
        );
    }

    #[benchmark]
    fn top_up_replica_sync_balance() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let replica_provider = create_provider::<T>(1);
        let bucket_id = setup_primary_agreement::<T>(&admin, &provider, 0);

        // Open the replica agreement via the signed-terms helper.
        setup_replica_agreement::<T>(&admin, bucket_id, &replica_provider, 1);

        let top_up_amount = BalanceOf::<T>::max_value() / 50u32.into();

        #[extrinsic_call]
        top_up_replica_sync_balance(
            RawOrigin::Signed(admin),
            bucket_id,
            replica_provider,
            top_up_amount,
        );
    }

    /// `on_initialize` slash sweep: drains and slashes every challenge expiring
    /// at a single deadline key. Linear in the challenge count `c`; each entry
    /// is drained, its pending counters decremented, and its provider slashed.
    /// The upper bound is the effective per-block slash budget
    /// `min(MaxChallengesPerDeadline, MAX_SWEEP_SLASH_BUDGET)` — the most the
    /// sweep ever slashes for one key in a block — so the linear fit covers the
    /// true worst case rather than extrapolating to it. The sweep applies this
    /// per key, so a small fixed hook overhead is counted here and again in the
    /// hook's base weight — conservative.
    #[benchmark]
    fn on_initialize_slash_challenges(
        c: Linear<
            0,
            {
                let cap = T::MaxChallengesPerDeadline::get() as u32;
                if cap < crate::pallet::MAX_SWEEP_SLASH_BUDGET {
                    cap
                } else {
                    crate::pallet::MAX_SWEEP_SLASH_BUDGET
                }
            },
        >,
    ) {
        let deadline: BlockNumberFor<T> = 200u32.into();
        let deposit: BalanceOf<T> = 100u32.into();
        for i in 0..c {
            // Distinct slashable provider (stake reserved) + challenger per
            // challenge — the worst case (each touches a distinct `Providers`,
            // `ChallengerStats`, and pending-counter entry).
            let provider = create_provider::<T>(i);
            let challenger = funded_account::<T>("challenger", i);
            // The slash unreserves the challenger's deposit, so reserve it.
            let _ = T::Currency::reserve(&challenger, deposit);
            let bucket_id: BucketId = i as u64;
            let challenge = pallet::Challenge::<T> {
                bucket_id,
                provider: provider.clone(),
                challenger,
                mmr_root: H256::zero(),
                start_seq: 0,
                target: ChunkLocation {
                    leaf_index: 0,
                    chunk_index: 0,
                },
                deposit,
            };
            Challenges::<T>::insert(deadline, i as u16, challenge);
            PendingChallenges::<T>::insert(&provider, 1u32);
            PendingChallengesByBucket::<T>::insert(bucket_id, &provider, 1u32);
        }
        NextChallengeIndex::<T>::insert(deadline, c as u16);

        // Drive the real sweep over exactly one key. Anchor the cursor one
        // below `deadline`, then set the relay clock so the sweepable range
        // (keys < previous relay parent) is exactly `{deadline}`:
        // `sweepable = current_anchor_block() - 1 = deadline`, `end = deadline`.
        LastSweptChallengeBlock::<T>::put(deadline.saturating_sub(1u32.into()));
        let now = deadline.saturating_add(1u32.into());
        set_block_number::<T>(now);

        #[block]
        {
            StorageProvider::<T>::on_initialize(now);
        }

        // Guard against the sweep silently no-op'ing (the bug this benchmark
        // had while it still called the dropped `on_finalize`): every challenge
        // at the deadline must have been drained. (At the worst-case component
        // `c == MaxChallengesPerDeadline` the slash budget is exactly spent, so
        // the cursor parks at `deadline - 1` and carries over — expected.)
        assert_eq!(Challenges::<T>::iter_prefix(deadline).count(), 0);
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
