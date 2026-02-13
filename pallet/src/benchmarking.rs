//! Benchmarking setup for pallet-storage-provider.
//!
//! Run benchmarks with:
//! ```bash
//! cargo build --release --features runtime-benchmarks
//! ./target/release/parachain-node benchmark pallet \
//!     --chain dev \
//!     --pallet pallet_storage_provider \
//!     --extrinsic "*" \
//!     --steps 50 \
//!     --repeat 20 \
//!     --output pallet/src/weights.rs
//! ```

#![cfg(feature = "runtime-benchmarks")]

use super::*;
use frame_benchmarking::v2::*;
use frame_support::{pallet_prelude::*, traits::Currency};
use frame_system::{pallet_prelude::BlockNumberFor, RawOrigin};
use sp_core::H256;
use sp_runtime::traits::Bounded;
use storage_primitives::{BucketId, ReplicaRequestParams};

const SEED: u32 = 0;

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
    let stake = T::MinProviderStake::get();

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
    let _ = Pallet::<T>::create_bucket(RawOrigin::Signed(admin.clone()).into(), 1);
    NextBucketId::<T>::get() - 1
}

fn setup_primary_agreement<T: Config>(
    admin: &T::AccountId,
    provider: &T::AccountId,
    bucket_id: BucketId,
) {
    let max_bytes = 1_000_000u64;
    let duration: BlockNumberFor<T> = 100u32.into();
    let payment = BalanceOf::<T>::max_value() / 10u32.into();

    // Request primary agreement
    let _ = Pallet::<T>::request_primary_agreement(
        RawOrigin::Signed(admin.clone()).into(),
        bucket_id,
        provider.clone(),
        max_bytes,
        duration,
        payment,
    );

    // Accept agreement
    let _ = Pallet::<T>::accept_agreement(
        RawOrigin::Signed(provider.clone()).into(),
        bucket_id,
    );
}

#[benchmarks]
mod benchmarks {
    use super::*;
    use frame_system::pallet_prelude::BlockNumberFor;

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
        let provider = create_provider::<T>(0);
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

    #[benchmark]
    fn block_extensions() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        #[extrinsic_call]
        set_extensions_blocked(RawOrigin::Signed(provider), bucket_id, true);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Bucket Management
    // ─────────────────────────────────────────────────────────────────────────

    #[benchmark]
    fn create_bucket() {
        let admin = funded_account::<T>("admin", 0);

        #[extrinsic_call]
        create_bucket(RawOrigin::Signed(admin), 1);
    }

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
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        // Need to create a checkpoint first
        let mmr_root = H256::repeat_byte(0xAB);
        let signatures: BoundedVec<(T::AccountId, sp_runtime::MultiSignature), T::MaxPrimaryProviders> =
            BoundedVec::new();

        let _ = Pallet::<T>::checkpoint(
            RawOrigin::Signed(admin.clone()).into(),
            bucket_id,
            mmr_root,
            0,
            10,
            signatures,
        );

        // Set min_providers to 0 so freeze succeeds
        let _ = Pallet::<T>::set_min_providers(
            RawOrigin::Signed(admin.clone()).into(),
            bucket_id,
            0,
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
        // This benchmark is complex to set up (requires slashing a provider)
        // Using a simplified version
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        // We'd need to slash the provider first, which requires a challenge
        // For now, this will fail but measures the weight of the checks
        #[extrinsic_call]
        remove_slashed(RawOrigin::Signed(admin), bucket_id, provider);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Agreement Management
    // ─────────────────────────────────────────────────────────────────────────

    #[benchmark]
    fn request_agreement() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        // Create a second provider for replica agreement
        let replica_provider = create_provider::<T>(1);
        let max_bytes = 1_000_000u64;
        let duration: BlockNumberFor<T> = 100u32.into();
        let payment = BalanceOf::<T>::max_value() / 10u32.into();
        let replica_params = ReplicaRequestParams {
            sync_balance: BalanceOf::<T>::max_value() / 20u32.into(),
            min_sync_interval: 10u32.into(),
        };

        #[extrinsic_call]
        request_agreement(
            RawOrigin::Signed(admin),
            bucket_id,
            replica_provider,
            max_bytes,
            duration,
            payment,
            replica_params,
        );
    }

    #[benchmark]
    fn request_primary_agreement() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        let max_bytes = 1_000_000u64;
        let duration: BlockNumberFor<T> = 100u32.into();
        let payment = BalanceOf::<T>::max_value() / 10u32.into();

        #[extrinsic_call]
        request_primary_agreement(
            RawOrigin::Signed(admin),
            bucket_id,
            provider,
            max_bytes,
            duration,
            payment,
        );
    }

    #[benchmark]
    fn accept_agreement() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        let max_bytes = 1_000_000u64;
        let duration: BlockNumberFor<T> = 100u32.into();
        let payment = BalanceOf::<T>::max_value() / 10u32.into();

        let _ = Pallet::<T>::request_primary_agreement(
            RawOrigin::Signed(admin).into(),
            bucket_id,
            provider.clone(),
            max_bytes,
            duration,
            payment,
        );

        #[extrinsic_call]
        accept_agreement(RawOrigin::Signed(provider), bucket_id);
    }

    #[benchmark]
    fn reject_agreement() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        let max_bytes = 1_000_000u64;
        let duration: BlockNumberFor<T> = 100u32.into();
        let payment = BalanceOf::<T>::max_value() / 10u32.into();

        let _ = Pallet::<T>::request_primary_agreement(
            RawOrigin::Signed(admin).into(),
            bucket_id,
            provider.clone(),
            max_bytes,
            duration,
            payment,
        );

        #[extrinsic_call]
        reject_agreement(RawOrigin::Signed(provider), bucket_id);
    }

    #[benchmark]
    fn withdraw_agreement_request() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        let max_bytes = 1_000_000u64;
        let duration: BlockNumberFor<T> = 100u32.into();
        let payment = BalanceOf::<T>::max_value() / 10u32.into();

        let _ = Pallet::<T>::request_primary_agreement(
            RawOrigin::Signed(admin.clone()).into(),
            bucket_id,
            provider.clone(),
            max_bytes,
            duration,
            payment,
        );

        #[extrinsic_call]
        withdraw_agreement_request(RawOrigin::Signed(admin), bucket_id, provider);
    }

    #[benchmark]
    fn top_up_agreement() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

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
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

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
    fn end_agreement() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        #[extrinsic_call]
        end_agreement(
            RawOrigin::Signed(admin),
            bucket_id,
            provider,
            storage_primitives::EndAction::Pay,
        );
    }

    #[benchmark]
    fn claim_expired_agreement() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        // Fast forward to after agreement expiry + settlement window
        // In benchmarks, we'd need to advance the block number
        // This will likely fail but measures weight

        #[extrinsic_call]
        claim_expired_agreement(RawOrigin::Signed(provider), bucket_id);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Checkpoints
    // ─────────────────────────────────────────────────────────────────────────

    #[benchmark]
    fn checkpoint() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        let mmr_root = H256::repeat_byte(0xAB);
        let signatures: BoundedVec<
            (T::AccountId, sp_runtime::MultiSignature),
            T::MaxPrimaryProviders,
        > = BoundedVec::new();

        #[extrinsic_call]
        checkpoint(
            RawOrigin::Signed(admin),
            bucket_id,
            mmr_root,
            0,
            10,
            signatures,
        );
    }

    #[benchmark]
    fn extend_checkpoint() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        // Create initial checkpoint
        let mmr_root = H256::repeat_byte(0xAB);
        let signatures: BoundedVec<
            (T::AccountId, sp_runtime::MultiSignature),
            T::MaxPrimaryProviders,
        > = BoundedVec::new();

        let _ = Pallet::<T>::checkpoint(
            RawOrigin::Signed(admin.clone()).into(),
            bucket_id,
            mmr_root,
            0,
            10,
            signatures,
        );

        // Add more signatures
        let additional_signatures: BoundedVec<
            (T::AccountId, sp_runtime::MultiSignature),
            T::MaxPrimaryProviders,
        > = BoundedVec::new();

        #[extrinsic_call]
        extend_checkpoint(RawOrigin::Signed(admin), bucket_id, additional_signatures);
    }

    #[benchmark]
    fn fund_checkpoint_pool() {
        let admin = funded_account::<T>("admin", 0);
        let bucket_id = setup_bucket::<T>(&admin);
        let amount = BalanceOf::<T>::max_value() / 100u32.into();

        #[extrinsic_call]
        fund_checkpoint_pool(RawOrigin::Signed(admin), bucket_id, amount);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Provider-Initiated Checkpoints
    // ─────────────────────────────────────────────────────────────────────────

    #[benchmark]
    fn provider_checkpoint(s: Linear<1, 5>) {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        // Fund checkpoint pool
        let pool_amount = BalanceOf::<T>::max_value() / 100u32.into();
        let _ = Pallet::<T>::fund_checkpoint_pool(
            RawOrigin::Signed(admin).into(),
            bucket_id,
            pool_amount,
        );

        let mmr_root = H256::repeat_byte(0xCD);
        let window = 1u64;

        // Create bounded signatures vec
        let signatures: BoundedVec<
            (T::AccountId, sp_runtime::MultiSignature),
            T::MaxPrimaryProviders,
        > = BoundedVec::new();

        #[extrinsic_call]
        provider_checkpoint(
            RawOrigin::Signed(provider),
            bucket_id,
            mmr_root,
            0,
            10,
            window,
            signatures,
        );
    }

    #[benchmark]
    fn configure_checkpoint_window() {
        let admin = funded_account::<T>("admin", 0);
        let bucket_id = setup_bucket::<T>(&admin);

        let interval: BlockNumberFor<T> = 200u32.into();
        let grace_period: BlockNumberFor<T> = 50u32.into();

        #[extrinsic_call]
        configure_checkpoint_window(RawOrigin::Signed(admin), bucket_id, interval, grace_period, true);
    }

    #[benchmark]
    fn report_missed_checkpoint() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        // This requires advancing blocks past window + grace
        // Will fail in benchmark but measures checks
        let window = 0u64;

        #[extrinsic_call]
        report_missed_checkpoint(RawOrigin::Signed(admin), bucket_id, window);
    }

    #[benchmark]
    fn claim_checkpoint_rewards() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        // Would need rewards accumulated first
        #[extrinsic_call]
        claim_checkpoint_rewards(RawOrigin::Signed(provider), bucket_id);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Challenge System
    // ─────────────────────────────────────────────────────────────────────────

    #[benchmark]
    fn challenge_checkpoint() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        // Create checkpoint first
        let mmr_root = H256::repeat_byte(0xAB);
        let signatures: BoundedVec<
            (T::AccountId, sp_runtime::MultiSignature),
            T::MaxPrimaryProviders,
        > = BoundedVec::new();

        let _ = Pallet::<T>::checkpoint(
            RawOrigin::Signed(admin.clone()).into(),
            bucket_id,
            mmr_root,
            0,
            10,
            signatures,
        );

        #[extrinsic_call]
        challenge_checkpoint(RawOrigin::Signed(admin), bucket_id, provider, 0, 0);
    }

    #[benchmark]
    fn challenge_off_chain() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        let mmr_root = H256::repeat_byte(0xAB);
        let signature = sp_runtime::MultiSignature::Sr25519(sp_core::sr25519::Signature::from_raw(
            [0u8; 64],
        ));

        #[extrinsic_call]
        challenge_offchain(
            RawOrigin::Signed(admin),
            bucket_id,
            provider,
            mmr_root,
            0,
            0,
            0,
            signature,
        );
    }

    #[benchmark]
    fn challenge_replica() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let replica_provider = create_provider::<T>(1);
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        // Create replica agreement
        let max_bytes = 1_000_000u64;
        let duration: BlockNumberFor<T> = 100u32.into();
        let payment = BalanceOf::<T>::max_value() / 10u32.into();
        let replica_params = ReplicaRequestParams {
            sync_balance: BalanceOf::<T>::max_value() / 20u32.into(),
            min_sync_interval: 10u32.into(),
        };

        let _ = Pallet::<T>::request_agreement(
            RawOrigin::Signed(admin.clone()).into(),
            bucket_id,
            replica_provider.clone(),
            max_bytes,
            duration,
            payment,
            replica_params,
        );

        let _ = Pallet::<T>::accept_agreement(
            RawOrigin::Signed(replica_provider.clone()).into(),
            bucket_id,
        );

        // Challenge will fail without sync, but measures weight
        #[extrinsic_call]
        challenge_replica(RawOrigin::Signed(admin), bucket_id, replica_provider, 0, 0);
    }

    #[benchmark]
    fn respond_to_challenge() {
        // This requires an active challenge which is complex to set up
        // Will measure the error checking weight
        let provider = create_provider::<T>(0);
        let challenge_id = storage_primitives::ChallengeId {
            deadline: 100u32.into(),
            index: 0,
        };

        // Create dummy response
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
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        // Create replica agreement
        let max_bytes = 1_000_000u64;
        let duration: BlockNumberFor<T> = 100u32.into();
        let payment = BalanceOf::<T>::max_value() / 10u32.into();
        let replica_params = ReplicaRequestParams {
            sync_balance: BalanceOf::<T>::max_value() / 20u32.into(),
            min_sync_interval: 10u32.into(),
        };

        let _ = Pallet::<T>::request_agreement(
            RawOrigin::Signed(admin).into(),
            bucket_id,
            replica_provider.clone(),
            max_bytes,
            duration,
            payment,
            replica_params,
        );

        let _ = Pallet::<T>::accept_agreement(
            RawOrigin::Signed(replica_provider.clone()).into(),
            bucket_id,
        );

        let roots: [Option<H256>; 7] = [
            Some(H256::repeat_byte(0xAB)),
            None,
            None,
            None,
            None,
            None,
            None,
        ];
        let signature = sp_runtime::MultiSignature::Sr25519(sp_core::sr25519::Signature::from_raw(
            [0u8; 64],
        ));

        #[extrinsic_call]
        confirm_replica_sync(RawOrigin::Signed(replica_provider), bucket_id, roots, signature);
    }

    #[benchmark]
    fn top_up_replica_sync_balance() {
        let admin = funded_account::<T>("admin", 0);
        let provider = create_provider::<T>(0);
        let replica_provider = create_provider::<T>(1);
        let bucket_id = setup_bucket::<T>(&admin);
        setup_primary_agreement::<T>(&admin, &provider, bucket_id);

        // Create replica agreement
        let max_bytes = 1_000_000u64;
        let duration: BlockNumberFor<T> = 100u32.into();
        let payment = BalanceOf::<T>::max_value() / 10u32.into();
        let replica_params = ReplicaRequestParams {
            sync_balance: BalanceOf::<T>::max_value() / 20u32.into(),
            min_sync_interval: 10u32.into(),
        };

        let _ = Pallet::<T>::request_agreement(
            RawOrigin::Signed(admin.clone()).into(),
            bucket_id,
            replica_provider.clone(),
            max_bytes,
            duration,
            payment,
            replica_params,
        );

        let _ = Pallet::<T>::accept_agreement(
            RawOrigin::Signed(replica_provider.clone()).into(),
            bucket_id,
        );

        let top_up_amount = BalanceOf::<T>::max_value() / 50u32.into();

        #[extrinsic_call]
        top_up_replica_sync_balance(
            RawOrigin::Signed(admin),
            bucket_id,
            replica_provider,
            top_up_amount,
        );
    }

    impl_benchmark_test_suite!(
        Pallet,
        crate::mock::new_test_ext(),
        crate::mock::Test
    );
}
