// SPDX-License-Identifier: GPL-3.0-only

//! Storage Parachain Runtime
//!
//! A minimal parachain runtime that includes the storage provider pallet
//! for decentralized storage with game-theoretic guarantees.

#![cfg_attr(not(feature = "std"), no_std)]
#![recursion_limit = "256"]

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

pub mod constants;
mod genesis_config_presets;
mod revive;
mod storage;
mod weights;
pub mod xcm_config;

extern crate alloc;

use alloc::borrow::Cow;
use alloc::{vec, vec::Vec};
use cumulus_pallet_parachain_system::RelayNumberMonotonicallyIncreases;
use cumulus_primitives_core::{AggregateMessageOrigin, ParaId};
use frame_support::{
    derive_impl,
    dispatch::DispatchClass,
    genesis_builder_helper::{build_state, get_preset},
    parameter_types,
    traits::{ConstBool, ConstU128, ConstU32, ConstU64, ConstU8, EitherOfDiverse, TransformOrigin},
    weights::{ConstantMultiplier, Weight},
    PalletId,
};
use frame_system::{
    limits::{BlockLength, BlockWeights},
    EnsureRoot,
};
use pallet_xcm::{EnsureXcm, IsVoiceOfBody};
use parachains_common::message_queue::{NarrowOriginToSibling, ParaIdToSibling};
use polkadot_runtime_common::xcm_sender::ExponentialPrice;
use sp_api::impl_runtime_apis;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_core::{crypto::KeyTypeId, OpaqueMetadata, H256};
use sp_runtime::{
    generic, impl_opaque_keys,
    traits::{BlakeTwo256, Block as BlockT, IdentifyAccount, Verify},
    transaction_validity::{TransactionSource, TransactionValidity},
    ApplyExtrinsicResult, MultiSignature,
};
use sp_version::RuntimeVersion;
#[cfg(feature = "runtime-benchmarks")]
use {constants::currency::UNIT, xcm_config::AssetHubLocation};

#[cfg(feature = "std")]
use sp_version::NativeVersion;

pub use frame_support::weights::constants::{
    BlockExecutionWeight, ExtrinsicBaseWeight, RocksDbWeight, WEIGHT_REF_TIME_PER_SECOND,
};
pub use frame_system::Call as SystemCall;
pub use pallet_balances::Call as BalancesCall;
pub use pallet_timestamp::Call as TimestampCall;
pub use sp_runtime::{Perbill, Permill};
use xcm::{prelude::*, Version as XcmVersion};
use xcm_runtime_apis::{
    dry_run::{CallDryRunEffects, Error as XcmDryRunApiError, XcmDryRunEffects},
    fees::Error as XcmPaymentApiError,
};

pub use pallet_storage_provider;

use constants::{
    consensus::{
        async_backing::UNINCLUDED_SEGMENT_CAPACITY, BLOCK_PROCESSING_VELOCITY,
        MAXIMUM_BLOCK_WEIGHT, RELAY_CHAIN_SLOT_DURATION_MILLIS, SLOT_DURATION,
    },
    currency::{EXISTENTIAL_DEPOSIT, MICROUNIT},
    system::{AVERAGE_ON_INITIALIZE_RATIO, NORMAL_DISPATCH_RATIO},
    time::HOURS,
};

/// Alias to 512-bit hash when used in the context of a transaction signature on the chain.
pub type Signature = MultiSignature;

/// Some way of identifying an account on the chain.
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;

/// Balance of an account.
pub type Balance = u128;

/// Index of a transaction in the chain.
pub type Nonce = u32;

/// A hash of some data used by the chain.
pub type Hash = H256;

/// An index to a block.
pub type BlockNumber = u32;

/// The address format for describing accounts.
pub type Address = sp_runtime::MultiAddress<AccountId, ()>;

/// Block header type as expected by this runtime.
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;

/// Block type as expected by this runtime.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;

/// A Block signed with a Justification
pub type SignedBlock = generic::SignedBlock<Block>;

/// BlockId type as expected by this runtime.
pub type BlockId = generic::BlockId<Block>;

/// The SignedExtension to the basic transaction logic.
pub type TxExtension = cumulus_pallet_weight_reclaim::StorageWeightReclaim<
    Runtime,
    (
        frame_system::CheckNonZeroSender<Runtime>,
        frame_system::CheckSpecVersion<Runtime>,
        frame_system::CheckTxVersion<Runtime>,
        frame_system::CheckGenesis<Runtime>,
        frame_system::CheckEra<Runtime>,
        frame_system::CheckNonce<Runtime>,
        frame_system::CheckWeight<Runtime>,
        pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
        frame_metadata_hash_extension::CheckMetadataHash<Runtime>,
        // lets the runtime accept Ethereum-signed transactions via `pallet_revive::eth_transact`
        pallet_revive::evm::tx_extension::SetOrigin<Runtime>,
    ),
>;

/// Unchecked extrinsic type as expected by this runtime.
///
/// Uses `pallet_revive`'s wrapper so the runtime accepts both substrate-signed
/// and Ethereum-signed (RLP/EIP-1559) transactions.
pub type UncheckedExtrinsic = pallet_revive::evm::runtime::UncheckedExtrinsic<
    Address,
    Signature,
    crate::revive::EthExtraImpl,
>;

/// Executive: handles dispatch to the various modules.
pub type Executive = frame_executive::Executive<
    Runtime,
    Block,
    frame_system::ChainContext<Runtime>,
    Runtime,
    AllPalletsWithSystem,
>;

/// `pallet_revive` requires this specific `WeightToFee` shape: a
/// `BlockRatioFee` parameterized by a per-cents target and an extrinsic-base
/// scale. The assumption is enforced at compile time when `pallet_revive::Config`
/// is implemented.
pub type WeightToFee = pallet_revive::evm::fees::BlockRatioFee<
    { crate::revive::CENTS },
    { 100 * ExtrinsicBaseWeight::get().ref_time() as u128 },
    Runtime,
    Balance,
>;

impl_opaque_keys! {
    pub struct SessionKeys {
        pub aura: Aura,
    }
}

#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
    spec_name: Cow::Borrowed("web3-storage-parachain"),
    impl_name: Cow::Borrowed("web3-storage-parachain"),
    authoring_version: 1,
    // Plain counter, not the semver encoding the paseo runtime uses. Zombienet
    // respawns this runtime from genesis, so no chain is ever upgraded across
    // these values.
    // * 1 on the initial parachain runtime;
    // * 2 for the breaking Challenges storage reshape (Vec -> StorageDoubleMap) (#125);
    // * 3 for dropping the vestigial `ChallengerStatRecord::total_earnings` field (#125);
    // * 4 for the `StorageProviderApi` additions, kept in lockstep with paseo's 4_004.
    spec_version: 4,
    impl_version: 0,
    apis: RUNTIME_API_VERSIONS,
    // Bumped whenever call encoding changes, so offline signers and stale-metadata
    // clients fail loudly rather than mis-encode a call.
    // * 1 on the initial parachain runtime;
    // * 2 for dropping the commitment nonce: `checkpoint` and `challenge_offchain` each lost
    //   a `nonce` argument, and `respond_to_challenge`'s `ChallengeResponse::Deleted` variant
    //   lost its `nonce` field (#339).
    transaction_version: 2,
    system_version: 1,
};

/// The version information used to identify this runtime when compiled natively.
#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
    NativeVersion {
        runtime_version: VERSION,
        can_author_with: Default::default(),
    }
}

/// Aura consensus hook
type ConsensusHook = cumulus_pallet_aura_ext::FixedVelocityConsensusHook<
    Runtime,
    RELAY_CHAIN_SLOT_DURATION_MILLIS,
    BLOCK_PROCESSING_VELOCITY,
    UNINCLUDED_SEGMENT_CAPACITY,
>;

parameter_types! {
    pub const BlockHashCount: BlockNumber = 4096;
    pub const Version: RuntimeVersion = VERSION;

    pub RuntimeBlockLength: BlockLength =
        BlockLength::builder()
            .max_length(5 * 1024 * 1024)
            .modify_max_length_for_class(
                DispatchClass::Normal,
                |m| *m = NORMAL_DISPATCH_RATIO * 5 * 1024 * 1024,
            )
            .build();
    pub RuntimeBlockWeights: BlockWeights = BlockWeights::builder()
        .base_block(BlockExecutionWeight::get())
        .for_class(DispatchClass::all(), |weights| {
            weights.base_extrinsic = ExtrinsicBaseWeight::get();
        })
        .for_class(DispatchClass::Normal, |weights| {
            weights.max_total = Some(NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT);
        })
        .for_class(DispatchClass::Operational, |weights| {
            weights.max_total = Some(MAXIMUM_BLOCK_WEIGHT);
            weights.reserved = Some(
                MAXIMUM_BLOCK_WEIGHT - NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT
            );
        })
        .avg_block_initialization(AVERAGE_ON_INITIALIZE_RATIO)
        .build_or_panic();
    pub const SS58Prefix: u16 = 42;
}

#[derive_impl(frame_system::config_preludes::ParaChainDefaultConfig)]
impl frame_system::Config for Runtime {
    type BaseCallFilter = frame_support::traits::Everything;
    type BlockWeights = RuntimeBlockWeights;
    type BlockLength = RuntimeBlockLength;
    type AccountId = AccountId;
    type RuntimeCall = RuntimeCall;
    type Lookup = sp_runtime::traits::AccountIdLookup<AccountId, ()>;
    type Nonce = Nonce;
    type Hash = Hash;
    type Hashing = BlakeTwo256;
    type Block = Block;
    type RuntimeEvent = RuntimeEvent;
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeTask = RuntimeTask;
    type BlockHashCount = BlockHashCount;
    type DbWeight = RocksDbWeight;
    type Version = Version;
    type PalletInfo = PalletInfo;
    type OnNewAccount = ();
    type OnKilledAccount = ();
    type AccountData = pallet_balances::AccountData<Balance>;
    type SystemWeightInfo = weights::frame_system::WeightInfo<Runtime>;
    type ExtensionsWeightInfo = weights::frame_system_extensions::WeightInfo<Runtime>;
    type SS58Prefix = SS58Prefix;
    type OnSetCode = cumulus_pallet_parachain_system::ParachainSetCode<Self>;
    type MaxConsumers = ConstU32<16>;
    type SingleBlockMigrations = ();
    type MultiBlockMigrator = ();
    type PreInherents = ();
    type PostInherents = ();
    type PostTransactions = ();
}

impl pallet_timestamp::Config for Runtime {
    type Moment = u64;
    type OnTimestampSet = Aura;
    type MinimumPeriod = ConstU64<0>;
    type WeightInfo = weights::pallet_timestamp::WeightInfo<Runtime>;
}

impl pallet_authorship::Config for Runtime {
    type FindAuthor = pallet_session::FindAccountFromAuthorIndex<Self, Aura>;
    type EventHandler = (CollatorSelection,);
}

parameter_types! {
    pub const ExistentialDeposit: Balance = EXISTENTIAL_DEPOSIT;
}

impl pallet_balances::Config for Runtime {
    type MaxLocks = ConstU32<50>;
    type Balance = Balance;
    type RuntimeEvent = RuntimeEvent;
    type DustRemoval = ();
    type ExistentialDeposit = ExistentialDeposit;
    type AccountStore = System;
    type WeightInfo = weights::pallet_balances::WeightInfo<Runtime>;
    type MaxReserves = ConstU32<50>;
    type ReserveIdentifier = [u8; 8];
    type RuntimeHoldReason = RuntimeHoldReason;
    type RuntimeFreezeReason = RuntimeFreezeReason;
    type FreezeIdentifier = RuntimeFreezeReason;
    type MaxFreezes = ConstU32<1>;
    type DoneSlashHandler = ();
}

parameter_types! {
    pub const TransactionByteFee: Balance = 10 * MICROUNIT;
}

impl pallet_transaction_payment::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type OnChargeTransaction = pallet_transaction_payment::FungibleAdapter<Balances, ()>;
    type WeightToFee = WeightToFee;
    type LengthToFee = ConstantMultiplier<Balance, TransactionByteFee>;
    // `pallet_revive`'s integrity check requires `FeeMultiplierUpdate::min()` to
    // be non-zero (the gas-price derivation multiplies through it). `()` returns
    // zero, so use the standard slow-adjusting curve from `polkadot-runtime-common`.
    type FeeMultiplierUpdate = polkadot_runtime_common::SlowAdjustingFeeUpdate<Self>;
    type OperationalFeeMultiplier = ConstU8<5>;
    type WeightInfo = weights::pallet_transaction_payment::WeightInfo<Runtime>;
}

impl pallet_utility::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type PalletsOrigin = OriginCaller;
    type WeightInfo = weights::pallet_utility::WeightInfo<Runtime>;
}

impl pallet_sudo::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type WeightInfo = weights::pallet_sudo::WeightInfo<Runtime>;
}

parameter_types! {
    pub const ReservedXcmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
    pub const ReservedDmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
    pub const RelayOrigin: AggregateMessageOrigin = AggregateMessageOrigin::Parent;
}

impl cumulus_pallet_parachain_system::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type OnSystemEvent = ();
    type SelfParaId = parachain_info::Pallet<Runtime>;
    type DmpQueue = frame_support::traits::EnqueueWithOrigin<MessageQueue, RelayOrigin>;
    type ReservedDmpWeight = ReservedDmpWeight;
    type OutboundXcmpMessageSource = XcmpQueue;
    type XcmpMessageHandler = XcmpQueue;
    type ReservedXcmpWeight = ReservedXcmpWeight;
    type CheckAssociatedRelayNumber = RelayNumberMonotonicallyIncreases;
    type ConsensusHook = ConsensusHook;
    type WeightInfo = weights::cumulus_pallet_parachain_system::WeightInfo<Runtime>;
    type RelayParentOffset = ConstU32<0>;
}

impl parachain_info::Config for Runtime {}

parameter_types! {
    pub MessageQueueServiceWeight: Weight = Perbill::from_percent(35) * RuntimeBlockWeights::get().max_block;
    pub AssetHubParaId: ParaId = ParaId::new(constants::system_parachain::ASSET_HUB_ID);
}

impl pallet_message_queue::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = weights::pallet_message_queue::WeightInfo<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type MessageProcessor =
        pallet_message_queue::mock_helpers::NoopMessageProcessor<AggregateMessageOrigin>;
    #[cfg(not(feature = "runtime-benchmarks"))]
    type MessageProcessor = xcm_builder::ProcessXcmMessage<
        AggregateMessageOrigin,
        xcm_executor::XcmExecutor<xcm_config::XcmConfig>,
        RuntimeCall,
    >;
    type Size = u32;
    type QueueChangeHandler = NarrowOriginToSibling<XcmpQueue>;
    type QueuePausedQuery = NarrowOriginToSibling<XcmpQueue>;
    type HeapSize = ConstU32<{ 103 * 1024 }>;
    type MaxStale = ConstU32<8>;
    type ServiceWeight = MessageQueueServiceWeight;
    type IdleMaxServiceWeight = MessageQueueServiceWeight;
}

impl cumulus_pallet_aura_ext::Config for Runtime {}

impl cumulus_pallet_xcmp_queue::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type ChannelInfo = ParachainSystem;
    type VersionWrapper = PolkadotXcm;
    type XcmpQueue = TransformOrigin<MessageQueue, AggregateMessageOrigin, ParaId, ParaIdToSibling>;
    type MaxInboundSuspended = ConstU32<1_000>;
    type MaxActiveOutboundChannels = ConstU32<128>;
    // Most on-chain HRMP channels are configured to use 102400 bytes of max message size, so we
    // need to set the page size larger than that until we reduce the channel size on-chain.
    type MaxPageSize = ConstU32<{ 103 * 1024 }>;
    type ControllerOrigin = RootOrFellows;
    type ControllerOriginConverter = xcm_config::XcmOriginToTransactDispatchOrigin;
    type PriceForSiblingDelivery = PriceForSiblingParachainDelivery;
    type WeightInfo = weights::cumulus_pallet_xcmp_queue::WeightInfo<Runtime>;
}

impl cumulus_pallet_xcmp_queue::migration::v5::V5Config for Runtime {
    type ChannelList = ParachainSystem;
}

parameter_types! {
    pub const Period: u32 = 6 * HOURS;
    pub const Offset: u32 = 0;
}

impl pallet_session::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type ValidatorId = <Self as frame_system::Config>::AccountId;
    type ValidatorIdOf = pallet_collator_selection::IdentityCollator;
    type ShouldEndSession = pallet_session::PeriodicSessions<Period, Offset>;
    type NextSessionRotation = pallet_session::PeriodicSessions<Period, Offset>;
    type SessionManager = CollatorSelection;
    type SessionHandler = <SessionKeys as sp_runtime::traits::OpaqueKeys>::KeyTypeIdProviders;
    type Keys = SessionKeys;
    type WeightInfo = weights::pallet_session::WeightInfo<Runtime>;
    type DisablingStrategy = pallet_session::disabling::UpToLimitDisablingStrategy;
    type Currency = Balances;
    type KeyDeposit = ConstU128<0>;
}

impl pallet_aura::Config for Runtime {
    type AuthorityId = AuraId;
    type DisabledValidators = ();
    type MaxAuthorities = ConstU32<100_000>;
    type AllowMultipleBlocksPerSlot = ConstBool<true>;
    type SlotDuration = ConstU64<SLOT_DURATION>;
}

parameter_types! {
    pub const PotId: PalletId = PalletId(*b"PotStake");
    pub const SessionLength: BlockNumber = 6 * HOURS;
    /// StakingAdmin pluralistic body.
    pub const StakingAdminBodyId: xcm::v5::BodyId = xcm::v5::BodyId::Defense;
    /// Fellows pluralistic body.
    pub const FellowsBodyId: xcm::v5::BodyId = xcm::v5::BodyId::Technical;
}

/// Privileged origin that represents Root or Fellows pluralistic body.
pub type RootOrFellows = EitherOfDiverse<
    EnsureRoot<AccountId>,
    EnsureXcm<IsVoiceOfBody<xcm_config::FellowshipLocation, FellowsBodyId>>,
>;

/// We allow Root and the StakingAdmin body on the governance chain to update collator selection.
pub type CollatorSelectionUpdateOrigin = EitherOfDiverse<
    EnsureRoot<AccountId>,
    EnsureXcm<IsVoiceOfBody<xcm_config::GovernanceLocation, StakingAdminBodyId>>,
>;

/// Exponential price for delivering XCM messages to sibling parachains.
pub type PriceForSiblingParachainDelivery = ExponentialPrice<
    xcm_config::FeeAssetId,
    xcm_config::BaseDeliveryFee,
    TransactionByteFee,
    XcmpQueue,
>;

impl pallet_collator_selection::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type UpdateOrigin = CollatorSelectionUpdateOrigin;
    type PotId = PotId;
    type MaxCandidates = ConstU32<100>;
    type MinEligibleCollators = ConstU32<4>;
    type MaxInvulnerables = ConstU32<20>;
    type KickThreshold = Period;
    type ValidatorId = <Self as frame_system::Config>::AccountId;
    type ValidatorIdOf = pallet_collator_selection::IdentityCollator;
    type ValidatorRegistration = Session;
    type WeightInfo = weights::pallet_collator_selection::WeightInfo<Runtime>;
}

impl cumulus_pallet_weight_reclaim::Config for Runtime {
    type WeightInfo = weights::cumulus_pallet_weight_reclaim::WeightInfo<Runtime>;
}

// Create the runtime by composing the FRAME pallets that were previously configured.
#[frame_support::runtime]
mod runtime {
    #[runtime::runtime]
    #[runtime::derive(
        RuntimeCall,
        RuntimeEvent,
        RuntimeError,
        RuntimeOrigin,
        RuntimeFreezeReason,
        RuntimeHoldReason,
        RuntimeSlashReason,
        RuntimeLockId,
        RuntimeTask
    )]
    pub struct Runtime;

    #[runtime::pallet_index(0)]
    pub type System = frame_system;

    #[runtime::pallet_index(1)]
    pub type ParachainSystem = cumulus_pallet_parachain_system;

    #[runtime::pallet_index(2)]
    pub type Timestamp = pallet_timestamp;

    #[runtime::pallet_index(3)]
    pub type ParachainInfo = parachain_info;

    // Monetary pallets
    #[runtime::pallet_index(10)]
    pub type Balances = pallet_balances;

    #[runtime::pallet_index(11)]
    pub type TransactionPayment = pallet_transaction_payment;

    // Governance
    #[runtime::pallet_index(15)]
    pub type Sudo = pallet_sudo;

    // Collator support
    #[runtime::pallet_index(20)]
    pub type Authorship = pallet_authorship;

    #[runtime::pallet_index(21)]
    pub type CollatorSelection = pallet_collator_selection;

    #[runtime::pallet_index(22)]
    pub type Session = pallet_session;

    #[runtime::pallet_index(23)]
    pub type Aura = pallet_aura;

    #[runtime::pallet_index(24)]
    pub type AuraExt = cumulus_pallet_aura_ext;

    // XCM
    #[runtime::pallet_index(30)]
    pub type XcmpQueue = cumulus_pallet_xcmp_queue;

    #[runtime::pallet_index(31)]
    pub type PolkadotXcm = pallet_xcm;

    #[runtime::pallet_index(32)]
    pub type CumulusXcm = cumulus_pallet_xcm;

    #[runtime::pallet_index(33)]
    pub type MessageQueue = pallet_message_queue;

    // Handy utilities. Utility / Multisig / Proxy / Indices ...
    #[runtime::pallet_index(34)]
    pub type Utility = pallet_utility;

    // Weight reclaim
    #[runtime::pallet_index(40)]
    pub type WeightReclaim = cumulus_pallet_weight_reclaim;

    // Storage Provider
    #[runtime::pallet_index(50)]
    pub type StorageProvider = pallet_storage_provider;

    // Drive Registry (Layer 1: File System)
    #[runtime::pallet_index(51)]
    pub type DriveRegistry = pallet_drive_registry;

    // S3 Registry (Layer 1: S3-Compatible Interface)
    #[runtime::pallet_index(52)]
    pub type S3Registry = pallet_s3_registry;

    // Smart contracts (PolkaVM + EVM-compatible)
    #[runtime::pallet_index(60)]
    pub type Revive = pallet_revive;
}

cumulus_pallet_parachain_system::register_validate_block! {
    Runtime = Runtime,
    BlockExecutor = cumulus_pallet_aura_ext::BlockExecutor::<Runtime, Executive>,
}

#[cfg(feature = "runtime-benchmarks")]
mod benches {
    frame_benchmarking::define_benchmarks!(
        [frame_system, SystemBench::<Runtime>]
        [frame_system_extensions, SystemExtensionsBench::<Runtime>]
        [cumulus_pallet_parachain_system, ParachainSystem]
        [pallet_timestamp, Timestamp]
        [pallet_balances, Balances]
        [pallet_transaction_payment, TransactionPayment]
        [pallet_collator_selection, CollatorSelection]
        [pallet_session, SessionBench::<Runtime>]
        [pallet_sudo, Sudo]
        [pallet_storage_provider, StorageProvider]
        [pallet_drive_registry, DriveRegistry]
        [pallet_s3_registry, S3Registry]
        [pallet_revive, Revive]
        [pallet_utility, Utility]
        [cumulus_pallet_xcmp_queue, XcmpQueue]
        [pallet_xcm, PalletXcmExtrinsicsBenchmark::<Runtime>]
        [pallet_message_queue, MessageQueue]
        // NOTE: these are individual modules of `pallet_xcm_benchmarks`.
        [pallet_xcm_benchmarks::fungible, XcmBalances]
        [pallet_xcm_benchmarks::generic, XcmGeneric]
        [cumulus_pallet_weight_reclaim, WeightReclaim]
    );
}

// Runtime API implementations.
//
// `pallet_revive::impl_runtime_apis_plus_revive_traits!` is a superset of the
// stock `impl_runtime_apis!` macro: it forwards the user-supplied impl blocks
// and additionally generates the revive-specific runtime APIs (`ReviveApi`,
// `eth_call`, `eth_transact`, etc.).
pallet_revive::impl_runtime_apis_plus_revive_traits!(
    Runtime,
    Revive,
    Executive,
    crate::revive::EthExtraImpl,

    impl sp_api::Core<Block> for Runtime {
        fn version() -> RuntimeVersion {
            VERSION
        }

        fn execute_block(block: <Block as BlockT>::LazyBlock) {
            Executive::execute_block(block)
        }

        fn initialize_block(header: &<Block as BlockT>::Header) -> sp_runtime::ExtrinsicInclusionMode {
            Executive::initialize_block(header)
        }
    }

    impl sp_api::Metadata<Block> for Runtime {
        fn metadata() -> OpaqueMetadata {
            OpaqueMetadata::new(Runtime::metadata().into())
        }

        fn metadata_at_version(version: u32) -> Option<OpaqueMetadata> {
            Runtime::metadata_at_version(version)
        }

        fn metadata_versions() -> Vec<u32> {
            Runtime::metadata_versions()
        }
    }

    impl sp_block_builder::BlockBuilder<Block> for Runtime {
        fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
            Executive::apply_extrinsic(extrinsic)
        }

        fn finalize_block() -> <Block as BlockT>::Header {
            Executive::finalize_block()
        }

        fn inherent_extrinsics(data: sp_inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
            data.create_extrinsics()
        }

        fn check_inherents(
            block: <Block as BlockT>::LazyBlock,
            data: sp_inherents::InherentData,
        ) -> sp_inherents::CheckInherentsResult {
            data.check_extrinsics(&block)
        }
    }

    impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
        fn validate_transaction(
            source: TransactionSource,
            tx: <Block as BlockT>::Extrinsic,
            block_hash: <Block as BlockT>::Hash,
        ) -> TransactionValidity {
            Executive::validate_transaction(source, tx, block_hash)
        }
    }

    impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
        fn offchain_worker(header: &<Block as BlockT>::Header) {
            Executive::offchain_worker(header)
        }
    }

    impl sp_session::SessionKeys<Block> for Runtime {
        fn generate_session_keys(owner: Vec<u8>, seed: Option<Vec<u8>>) -> sp_session::OpaqueGeneratedSessionKeys {
            SessionKeys::generate(&owner, seed).into()
        }

        fn decode_session_keys(
            encoded: Vec<u8>,
        ) -> Option<Vec<(Vec<u8>, KeyTypeId)>> {
            SessionKeys::decode_into_raw_public_keys(&encoded)
        }
    }

    impl sp_consensus_aura::AuraApi<Block, AuraId> for Runtime {
        fn slot_duration() -> sp_consensus_aura::SlotDuration {
            sp_consensus_aura::SlotDuration::from_millis(SLOT_DURATION)
        }

        fn authorities() -> Vec<AuraId> {
            pallet_aura::Authorities::<Runtime>::get().into_inner()
        }
    }

    impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce> for Runtime {
        fn account_nonce(account: AccountId) -> Nonce {
            System::account_nonce(account)
        }
    }

    impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance> for Runtime {
        fn query_info(
            uxt: <Block as BlockT>::Extrinsic,
            len: u32,
        ) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
            TransactionPayment::query_info(uxt, len)
        }
        fn query_fee_details(
            uxt: <Block as BlockT>::Extrinsic,
            len: u32,
        ) -> pallet_transaction_payment::FeeDetails<Balance> {
            TransactionPayment::query_fee_details(uxt, len)
        }
        fn query_weight_to_fee(weight: Weight) -> Balance {
            TransactionPayment::weight_to_fee(weight)
        }
        fn query_length_to_fee(length: u32) -> Balance {
            TransactionPayment::length_to_fee(length)
        }
    }

    impl cumulus_primitives_core::CollectCollationInfo<Block> for Runtime {
        fn collect_collation_info(header: &<Block as BlockT>::Header) -> cumulus_primitives_core::CollationInfo {
            ParachainSystem::collect_collation_info(header)
        }
    }

    impl cumulus_primitives_aura::AuraUnincludedSegmentApi<Block> for Runtime {
        fn can_build_upon(
            included_hash: <Block as BlockT>::Hash,
            slot: cumulus_primitives_aura::Slot,
        ) -> bool {
            ConsensusHook::can_build_upon(included_hash, slot)
        }
    }

    impl cumulus_primitives_core::RelayParentOffsetApi<Block> for Runtime {
        fn relay_parent_offset() -> u32 {
            0
        }
    }

    impl cumulus_primitives_core::GetParachainInfo<Block> for Runtime {
        fn parachain_id() -> ParaId {
            ParachainInfo::parachain_id()
        }
    }

    impl sp_genesis_builder::GenesisBuilder<Block> for Runtime {
        fn build_state(config: Vec<u8>) -> sp_genesis_builder::Result {
            build_state::<RuntimeGenesisConfig>(config)
        }

        fn get_preset(id: &Option<sp_genesis_builder::PresetId>) -> Option<Vec<u8>> {
            get_preset::<RuntimeGenesisConfig>(id, &genesis_config_presets::get_preset)
        }

        fn preset_names() -> Vec<sp_genesis_builder::PresetId> {
            genesis_config_presets::preset_names()
        }
    }

    // The API's `BlockNumber` slot carries anchor-clock values (`challenges_at`
    // deadlines, `current_anchor_block`), so it is instantiated with the
    // pallet's anchor-denominated `BlockNumberFor` — the same concrete type as
    // the runtime's `BlockNumber`, per the pallet's `Config` pin.
    impl pallet_storage_provider::runtime_api::StorageProviderApi<Block, AccountId, pallet_storage_provider::BlockNumberFor<Runtime>, Balance> for Runtime {
        fn provider_info(provider: AccountId) -> Option<pallet_storage_provider::runtime_api::ProviderInfoResponse> {
            StorageProvider::query_provider_info(&provider)
        }

        fn providers(offset: u32, limit: u32) -> Vec<(AccountId, pallet_storage_provider::runtime_api::ProviderInfoResponse)> {
            StorageProvider::query_providers(offset, limit)
        }

        fn bucket_info(bucket_id: storage_primitives::BucketId) -> Option<pallet_storage_provider::runtime_api::BucketResponse> {
            StorageProvider::query_bucket_info(bucket_id)
        }

        fn bucket_ids(offset: u32, limit: u32) -> Vec<storage_primitives::BucketId> {
            StorageProvider::query_bucket_ids(offset, limit)
        }

        fn bucket_providers(bucket_id: storage_primitives::BucketId) -> Vec<AccountId> {
            StorageProvider::query_bucket_providers(bucket_id)
        }

        fn agreement_info(bucket_id: storage_primitives::BucketId, provider: AccountId) -> Option<pallet_storage_provider::runtime_api::AgreementResponse> {
            StorageProvider::query_agreement_info(bucket_id, &provider)
        }

        fn bucket_agreements(bucket_id: storage_primitives::BucketId) -> Vec<pallet_storage_provider::runtime_api::AgreementResponse> {
            StorageProvider::query_bucket_agreements(bucket_id)
        }

        fn provider_agreements(provider: AccountId) -> Vec<pallet_storage_provider::runtime_api::AgreementResponse> {
            StorageProvider::query_provider_agreements(&provider)
        }

        fn challenges_at(block: pallet_storage_provider::BlockNumberFor<Runtime>) -> Vec<pallet_storage_provider::runtime_api::ChallengeResponse> {
            StorageProvider::query_challenges_at(block)
        }

        fn bucket_challenges(bucket_id: storage_primitives::BucketId) -> Vec<pallet_storage_provider::runtime_api::ChallengeResponse> {
            StorageProvider::query_bucket_challenges(bucket_id)
        }

        fn provider_challenges(provider: AccountId) -> Vec<pallet_storage_provider::runtime_api::ChallengeResponse> {
            StorageProvider::query_provider_challenges(&provider)
        }

        fn challenger_challenges(challenger: AccountId) -> Vec<pallet_storage_provider::runtime_api::ChallengeResponse> {
            StorageProvider::query_challenger_challenges(&challenger)
        }

        fn can_accept_bytes(provider: AccountId, additional_bytes: u64) -> bool {
            StorageProvider::query_can_accept_bytes(&provider, additional_bytes)
        }

        fn find_matching_providers(
            requirements: pallet_storage_provider::runtime_api::StorageRequirements,
            limit: u32,
        ) -> Vec<pallet_storage_provider::runtime_api::MatchedProvider> {
            StorageProvider::query_find_matching_providers(requirements, limit)
        }

        fn providers_with_capacity(
            bytes_needed: u64,
            offset: u32,
            limit: u32,
        ) -> Vec<(AccountId, pallet_storage_provider::runtime_api::ProviderInfoResponse)> {
            StorageProvider::query_providers_with_capacity(bytes_needed, offset, limit)
        }

        fn challenge_candidates(
            max_reputation: u8,
            limit: u32,
        ) -> Vec<pallet_storage_provider::runtime_api::ChallengeCandidate> {
            StorageProvider::query_challenge_candidates(max_reputation, limit)
        }

        fn current_anchor_block() -> pallet_storage_provider::BlockNumberFor<Runtime> {
            StorageProvider::current_anchor_block()
        }

        fn anchor_block_time_millis() -> u64 {
            StorageProvider::anchor_block_time_millis()
        }
    }

    impl xcm_runtime_apis::fees::XcmPaymentApi<Block> for Runtime {
        fn query_acceptable_payment_assets(xcm_version: xcm::Version) -> Result<Vec<VersionedAssetId>, XcmPaymentApiError> {
            let acceptable_assets = alloc::vec![AssetId(xcm_config::RelayLocation::get())];
            PolkadotXcm::query_acceptable_payment_assets(xcm_version, acceptable_assets)
        }

        fn query_weight_to_asset_fee(weight: Weight, asset: VersionedAssetId) -> Result<u128, XcmPaymentApiError> {
            use crate::xcm_config::XcmConfig;

            type Trader = <XcmConfig as xcm_executor::Config>::Trader;

            PolkadotXcm::query_weight_to_asset_fee::<Trader>(weight, asset)
        }

        fn query_xcm_weight(message: VersionedXcm<()>) -> Result<Weight, XcmPaymentApiError> {
            PolkadotXcm::query_xcm_weight(message)
        }

        fn query_delivery_fees(destination: VersionedLocation, message: VersionedXcm<()>, asset_id: VersionedAssetId) -> Result<VersionedAssets, XcmPaymentApiError> {
            type AssetExchanger = <xcm_config::XcmConfig as xcm_executor::Config>::AssetExchanger;
            PolkadotXcm::query_delivery_fees::<AssetExchanger>(destination, message, asset_id)
        }
    }

    impl xcm_runtime_apis::dry_run::DryRunApi<Block, RuntimeCall, RuntimeEvent, OriginCaller> for Runtime {
        fn dry_run_call(origin: OriginCaller, call: RuntimeCall, result_xcms_version: XcmVersion) -> Result<CallDryRunEffects<RuntimeEvent>, XcmDryRunApiError> {
            PolkadotXcm::dry_run_call::<Runtime, xcm_config::XcmRouter, OriginCaller, RuntimeCall>(origin, call, result_xcms_version)
        }

        fn dry_run_xcm(origin_location: VersionedLocation, xcm: VersionedXcm<RuntimeCall>) -> Result<XcmDryRunEffects<RuntimeEvent>, XcmDryRunApiError> {
            PolkadotXcm::dry_run_xcm::<xcm_config::XcmRouter>(origin_location, xcm)
        }
    }

    impl xcm_runtime_apis::conversions::LocationToAccountApi<Block, AccountId> for Runtime {
        fn convert_location(location: VersionedLocation) -> Result<
            AccountId,
            xcm_runtime_apis::conversions::Error
        > {
            xcm_runtime_apis::conversions::LocationToAccountHelper::<
                AccountId,
                xcm_config::LocationToAccountId,
            >::convert_location(location)
        }
    }

    impl xcm_runtime_apis::trusted_query::TrustedQueryApi<Block> for Runtime {
        fn is_trusted_reserve(asset: VersionedAsset, location: VersionedLocation) -> xcm_runtime_apis::trusted_query::XcmTrustedQueryResult {
            PolkadotXcm::is_trusted_reserve(asset, location)
        }
        fn is_trusted_teleporter(asset: VersionedAsset, location: VersionedLocation) -> xcm_runtime_apis::trusted_query::XcmTrustedQueryResult {
            PolkadotXcm::is_trusted_teleporter(asset, location)
        }
    }

    impl xcm_runtime_apis::authorized_aliases::AuthorizedAliasersApi<Block> for Runtime {
        fn authorized_aliasers(target: VersionedLocation) -> Result<
            Vec<xcm_runtime_apis::authorized_aliases::OriginAliaser>,
            xcm_runtime_apis::authorized_aliases::Error
        > {
            PolkadotXcm::authorized_aliasers(target)
        }
        fn is_authorized_alias(origin: VersionedLocation, target: VersionedLocation) -> Result<
            bool,
            xcm_runtime_apis::authorized_aliases::Error
        > {
            PolkadotXcm::is_authorized_alias(origin, target)
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    impl frame_benchmarking::Benchmark<Block> for Runtime {
        fn benchmark_metadata(extra: bool) -> (
            Vec<frame_benchmarking::BenchmarkList>,
            Vec<frame_support::traits::StorageInfo>,
        ) {
            use cumulus_pallet_session_benchmarking::Pallet as SessionBench;
            use frame_benchmarking::BenchmarkList;
            use frame_support::traits::StorageInfoTrait;
            use frame_system_benchmarking::{
                extensions::Pallet as SystemExtensionsBench, Pallet as SystemBench,
            };
            use pallet_xcm::benchmarking::Pallet as PalletXcmExtrinsicsBenchmark;

            // The XCM benchmarks module pallets are referenced by `list_benchmarks!`
            // and `add_benchmarks!`, so they need to be in scope for both calls.
            type XcmBalances = pallet_xcm_benchmarks::fungible::Pallet<Runtime>;
            type XcmGeneric = pallet_xcm_benchmarks::generic::Pallet<Runtime>;

            let mut list = Vec::<BenchmarkList>::new();
            list_benchmarks!(list, extra);

            let storage_info = AllPalletsWithSystem::storage_info();
            (list, storage_info)
        }

        #[allow(non_local_definitions)]
        fn dispatch_benchmark(
            config: frame_benchmarking::BenchmarkConfig,
        ) -> Result<Vec<frame_benchmarking::BenchmarkBatch>, alloc::string::String> {
            use codec::Encode;
            use frame_benchmarking::{BenchmarkBatch, BenchmarkError};
            use sp_storage::TrackedStorageKey;

            use frame_system_benchmarking::{
                extensions::Pallet as SystemExtensionsBench, Pallet as SystemBench,
            };
            impl frame_system_benchmarking::Config for Runtime {
                fn setup_set_code_requirements(
                    code: &alloc::vec::Vec<u8>,
                ) -> Result<(), BenchmarkError> {
                    ParachainSystem::initialize_for_set_code_benchmark(code.len() as u32);
                    Ok(())
                }

                fn verify_set_code() {
                    System::assert_last_event(
                        cumulus_pallet_parachain_system::Event::<Runtime>::ValidationFunctionStored
                            .into(),
                    );
                }
            }

            use cumulus_pallet_session_benchmarking::Pallet as SessionBench;
            impl cumulus_pallet_session_benchmarking::Config for Runtime {
                fn generate_session_keys_and_proof(
                    owner: Self::AccountId,
                ) -> (Self::Keys, Vec<u8>) {
                    let keys = SessionKeys::generate(&owner.encode(), None);
                    (keys.keys, keys.proof.encode())
                }
            }

            impl pallet_transaction_payment::BenchmarkConfig for Runtime {}

            use alloc::boxed::Box;
            use xcm_executor::AssetsInHolding;

            use pallet_xcm::benchmarking::Pallet as PalletXcmExtrinsicsBenchmark;
            impl pallet_xcm::benchmarking::Config for Runtime {
                type DeliveryHelper =
                    polkadot_runtime_common::xcm_sender::ToParachainDeliveryHelper<
                        xcm_config::XcmConfig,
                        ExistentialDepositAsset,
                        PriceForSiblingParachainDelivery,
                        AssetHubParaId,
                        ParachainSystem,
                    >;

                fn reachable_dest() -> Option<Location> {
                    Some(xcm_config::AssetHubLocation::get())
                }

                fn teleportable_asset_and_dest() -> Option<(Asset, Location)> {
                    Some((
                        Asset {
                            fun: Fungible(ExistentialDeposit::get()),
                            id: AssetId(Parent.into()),
                        },
                        xcm_config::AssetHubLocation::get(),
                    ))
                }

                fn reserve_transferable_asset_and_dest() -> Option<(Asset, Location)> {
                    None
                }

                fn set_up_complex_asset_transfer(
                ) -> Option<(Assets, u32, Location, alloc::boxed::Box<dyn FnOnce()>)> {
                    let native_location = Parent.into();
                    let dest = xcm_config::AssetHubLocation::get();
                    pallet_xcm::benchmarking::helpers::native_teleport_as_asset_transfer::<Runtime>(
                        native_location,
                        dest,
                    )
                }

                fn get_asset() -> Asset {
                    Asset {
                        id: AssetId(Location::parent()),
                        fun: Fungible(ExistentialDeposit::get()),
                    }
                }
            }

            parameter_types! {
                pub ExistentialDepositAsset: Option<Asset> = Some((
                    xcm_config::RelayLocation::get(),
                    ExistentialDeposit::get()
                ).into());
            }

            impl pallet_xcm_benchmarks::Config for Runtime {
                type XcmConfig = xcm_config::XcmConfig;
                type AccountIdConverter = xcm_config::LocationToAccountId;
                type DeliveryHelper =
                    polkadot_runtime_common::xcm_sender::ToParachainDeliveryHelper<
                        xcm_config::XcmConfig,
                        ExistentialDepositAsset,
                        PriceForSiblingParachainDelivery,
                        AssetHubParaId,
                        ParachainSystem,
                    >;

                fn valid_destination() -> Result<Location, BenchmarkError> {
                    Ok(xcm_config::AssetHubLocation::get())
                }

                fn worst_case_holding(_depositable_count: u32) -> AssetsInHolding {
                    use pallet_xcm_benchmarks::MockCredit;
                    let mut holding = AssetsInHolding::new();
                    holding.fungible.insert(
                        AssetId(xcm_config::RelayLocation::get()),
                        Box::new(MockCredit(1_000_000 * UNIT)),
                    );
                    holding
                }
            }

            parameter_types! {
                pub TrustedTeleporter: Option<(Location, Asset)> = Some((
                    AssetHubLocation::get(),
                    Asset { fun: Fungible(UNIT), id: AssetId(xcm_config::RelayLocation::get()) },
                ));
                pub const CheckedAccount: Option<(AccountId, xcm_builder::MintLocation)> = None;
                pub const TrustedReserve: Option<(Location, Asset)> = None;
            }

            impl pallet_xcm_benchmarks::fungible::Config for Runtime {
                type TransactAsset = Balances;

                type CheckedAccount = CheckedAccount;
                type TrustedTeleporter = TrustedTeleporter;
                type TrustedReserve = TrustedReserve;

                fn get_asset() -> Asset {
                    Asset {
                        id: AssetId(xcm_config::RelayLocation::get()),
                        fun: Fungible(UNIT),
                    }
                }
            }

            impl pallet_xcm_benchmarks::generic::Config for Runtime {
                type RuntimeCall = RuntimeCall;
                type TransactAsset = Balances;

                fn worst_case_response() -> (u64, Response) {
                    (0u64, Response::Version(Default::default()))
                }

                fn worst_case_asset_exchange() -> Result<(Assets, Assets), BenchmarkError> {
                    Err(BenchmarkError::Skip)
                }

                fn universal_alias() -> Result<(Location, Junction), BenchmarkError> {
                    Err(BenchmarkError::Skip)
                }

                fn transact_origin_and_runtime_call(
                ) -> Result<(Location, RuntimeCall), BenchmarkError> {
                    Ok((
                        xcm_config::AssetHubLocation::get(),
                        frame_system::Call::remark_with_event { remark: alloc::vec![] }.into(),
                    ))
                }

                fn subscribe_origin() -> Result<Location, BenchmarkError> {
                    Ok(xcm_config::AssetHubLocation::get())
                }

                fn claimable_asset() -> Result<(Location, Location, Assets), BenchmarkError> {
                    let origin = xcm_config::AssetHubLocation::get();
                    let assets: Assets =
                        (AssetId(xcm_config::RelayLocation::get()), 1_000 * UNIT).into();
                    let ticket = Location { parents: 0, interior: Here };
                    Ok((origin, ticket, assets))
                }

                fn worst_case_for_trader() -> Result<(Asset, WeightLimit), BenchmarkError> {
                    Ok((
                        Asset {
                            id: AssetId(xcm_config::RelayLocation::get()),
                            fun: Fungible(1_000_000 * UNIT),
                        },
                        WeightLimit::Limited(Weight::from_parts(5000, 5000)),
                    ))
                }

                fn unlockable_asset() -> Result<(Location, Location, Asset), BenchmarkError> {
                    Err(BenchmarkError::Skip)
                }

                fn export_message_origin_and_destination(
                ) -> Result<(Location, NetworkId, InteriorLocation), BenchmarkError> {
                    Err(BenchmarkError::Skip)
                }

                fn alias_origin() -> Result<(Location, Location), BenchmarkError> {
                    Ok((
                        Location::new(1, [Parachain(1000)]),
                        Location::new(
                            1,
                            [
                                Parachain(1000),
                                AccountId32 { id: [111u8; 32], network: None },
                            ],
                        ),
                    ))
                }
            }

            type XcmBalances = pallet_xcm_benchmarks::fungible::Pallet<Runtime>;
            type XcmGeneric = pallet_xcm_benchmarks::generic::Pallet<Runtime>;

            use frame_support::traits::WhitelistedStorageKeys;
            let whitelist: Vec<TrackedStorageKey> =
                AllPalletsWithSystem::whitelisted_storage_keys();

            let mut batches = Vec::<BenchmarkBatch>::new();
            let params = (&config, &whitelist);
            add_benchmarks!(params, batches);

            Ok(batches)
        }
    }

    #[cfg(feature = "try-runtime")]
    impl frame_try_runtime::TryRuntime<Block> for Runtime {
        fn on_runtime_upgrade(checks: frame_try_runtime::UpgradeCheckSelect) -> (Weight, Weight) {
            let weight = Executive::try_runtime_upgrade(checks).unwrap();
            (weight, RuntimeBlockWeights::get().max_block)
        }

        fn execute_block(
            block: sp_runtime::generic::LazyBlock<Header, UncheckedExtrinsic>,
            state_root_check: bool,
            signature_check: bool,
            select: frame_try_runtime::TryStateSelect,
        ) -> Weight {
            Executive::try_execute_block(block, state_root_check, signature_check, select).unwrap()
        }
    }
);
