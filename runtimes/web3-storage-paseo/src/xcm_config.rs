// SPDX-License-Identifier: GPL-3.0-only

//! XCM configuration for the Paseo Web3 Storage parachain.

use super::{
    AccountId, AllPalletsWithSystem, Balance, Balances, ParachainInfo, ParachainSystem,
    PolkadotXcm, Runtime, RuntimeCall, RuntimeEvent, RuntimeHoldReason, RuntimeOrigin,
    TransactionByteFee, WeightToFee, XcmpQueue,
};
use crate::paseo_constants::system_parachain::{ASSET_HUB_ID, COLLECTIVES_ID};
use frame_support::{
    parameter_types,
    traits::{
        fungible::HoldConsideration, tokens::imbalance::ResolveTo, ConstU32, Contains, Equals,
        Everything, LinearStoragePrice, Nothing,
    },
    weights::Weight,
};
use frame_system::EnsureRoot;
use pallet_collator_selection::StakingPotAccountId;
use pallet_xcm::{AuthorizedAliasers, XcmPassthrough};
use parachains_common::{
    xcm_config::{
        AliasAccountId32FromSiblingSystemChain, AllSiblingSystemParachains,
        ConcreteAssetFromSystem, ParentRelayOrSiblingParachains, RelayOrOtherSystemParachains,
    },
    TREASURY_PALLET_ID,
};
use polkadot_parachain_primitives::primitives::Sibling;
use polkadot_runtime_common::xcm_sender::ExponentialPrice;
use xcm::latest::prelude::*;
use xcm_builder::{
    AccountId32Aliases, AliasChildLocation, AliasOriginRootUsingFilter,
    AllowExplicitUnpaidExecutionFrom, AllowHrmpNotificationsFromRelayChain,
    AllowKnownQueryResponses, AllowSubscriptionsFrom, AllowTopLevelPaidExecutionFrom,
    DescribeAllTerminal, DescribeFamily, EnsureXcmOrigin, FrameTransactionalProcessor,
    FungibleAdapter, HashedDescription, IsConcrete, LocationAsSuperuser, ParentIsPreset,
    RelayChainAsNative, SendXcmFeeToAccount, SiblingParachainAsNative, SiblingParachainConvertsVia,
    SignedAccountId32AsNative, SignedToAccountId32, SovereignSignedViaLocation, TakeWeightCredit,
    TrailingSetTopicAsId, UsingComponents, WeightInfoBounds, WithComputedOrigin, WithUniqueTopic,
    XcmFeeManagerFromComponents,
};
use xcm_executor::XcmExecutor;

// Re-export
pub use crate::paseo_constants::locations::{GovernanceLocation, PeopleLocation};

/// The treasury account expressed as an XCM `Location` (local AccountId32 junction).
/// `SendXcmFeeToAccount` requires `Get<Location>`, so we wrap the same `py/trsry` pallet ID
/// that lib.rs uses for `TreasuryAccount`.
pub struct TreasuryLocation;
impl frame_support::traits::Get<Location> for TreasuryLocation {
    fn get() -> Location {
        let id: [u8; 32] =
            sp_runtime::traits::AccountIdConversion::<AccountId>::into_account_truncating(
                &TREASURY_PALLET_ID,
            )
            .into();
        AccountId32 { network: None, id }.into()
    }
}

/// The genesis hash of the Paseo testnet relay chain. Used to identify it over XCM.
///
/// Not yet exposed by the Polkadot SDK, so we define it locally. Matches
/// `0x77afd6190f1554ad45fd0d31aee62aacc33c6db0ea801129acb813f913e0764f`.
pub const PASEO_GENESIS_HASH: [u8; 32] = [
    0x77, 0xaf, 0xd6, 0x19, 0x0f, 0x15, 0x54, 0xad, 0x45, 0xfd, 0x0d, 0x31, 0xae, 0xe6, 0x2a, 0xac,
    0xc3, 0x3c, 0x6d, 0xb0, 0xea, 0x80, 0x11, 0x29, 0xac, 0xb8, 0x13, 0xf9, 0x13, 0xe0, 0x76, 0x4f,
];

parameter_types! {
    pub const RootLocation: Location = Location::here();
    pub const RelayLocation: Location = Location::parent();
    pub AssetHubLocation: Location = Location::new(1, [Parachain(ASSET_HUB_ID)]);
    pub const RelayNetwork: Option<NetworkId> = Some(NetworkId::ByGenesis(PASEO_GENESIS_HASH));
    pub RelayChainOrigin: RuntimeOrigin = cumulus_pallet_xcm::Origin::Relay.into();
    pub UniversalLocation: InteriorLocation = [
        GlobalConsensus(RelayNetwork::get().unwrap()),
        Parachain(ParachainInfo::parachain_id().into())
    ].into();
    pub const MaxInstructions: u32 = 100;
    pub const MaxAssetsIntoHolding: u32 = 64;
    pub FellowshipLocation: Location = Location::new(1, Parachain(COLLECTIVES_ID));
    pub FeeAssetId: AssetId = AssetId(RelayLocation::get());
    pub const BaseDeliveryFee: u128 = 3 * crate::paseo_constants::currency::MILLIUNIT;
    pub const DepositPerItem: Balance = crate::paseo_constants::currency::MILLIUNIT;
    pub const DepositPerByte: Balance = crate::paseo_constants::currency::MICROUNIT;
    pub const AuthorizeAliasHoldReason: RuntimeHoldReason =
        RuntimeHoldReason::PolkadotXcm(pallet_xcm::HoldReason::AuthorizeAlias);
}

/// Type for specifying how a `Location` can be converted into an `AccountId`.
pub type LocationToAccountId = (
    // The parent (Relay-chain) origin converts to the parent `AccountId`.
    ParentIsPreset<AccountId>,
    // Sibling parachain origins convert to AccountId via the `ParaId::into`.
    SiblingParachainConvertsVia<Sibling, AccountId>,
    // Straight up local `AccountId32` origins just alias directly to `AccountId`.
    AccountId32Aliases<RelayNetwork, AccountId>,
    // Foreign locations alias into accounts according to a hash of their standard description.
    HashedDescription<AccountId, DescribeFamily<DescribeAllTerminal>>,
);

/// Means for transacting assets on this chain.
pub type LocalAssetTransactor = FungibleAdapter<
    // Use this currency:
    Balances,
    // Use this currency when it is a fungible asset matching the given location or name:
    IsConcrete<RelayLocation>,
    // Do a simple punn to convert an AccountId32 Location into a native chain account ID:
    LocationToAccountId,
    // Our chain's account ID type (we can't get away without mentioning it explicitly):
    AccountId,
    // We don't track any teleports.
    (),
>;

/// This is the type we use to convert an (incoming) XCM origin into a local `Origin` instance.
pub type XcmOriginToTransactDispatchOrigin = (
    // Governance location (Asset Hub) can gain root.
    LocationAsSuperuser<Equals<GovernanceLocation>, RuntimeOrigin>,
    // Sovereign account converter; this attempts to derive an `AccountId` from the origin location
    // using `LocationToAccountId` and then turn that into the usual `Signed` origin.
    SovereignSignedViaLocation<LocationToAccountId, RuntimeOrigin>,
    // Native converter for Relay-chain (Parent) location; will convert to a `Relay` origin when
    // recognized.
    RelayChainAsNative<RelayChainOrigin, RuntimeOrigin>,
    // Native converter for sibling Parachains; will convert to a `SiblingPara` origin when
    // recognized.
    SiblingParachainAsNative<cumulus_pallet_xcm::Origin, RuntimeOrigin>,
    // Native signed account converter; this just converts an `AccountId32` origin into a normal
    // `RuntimeOrigin::Signed` origin of the same 32-byte value.
    SignedAccountId32AsNative<RelayNetwork, RuntimeOrigin>,
    // Xcm origins can be represented natively under the Xcm pallet's Xcm origin.
    XcmPassthrough<RuntimeOrigin>,
);

pub struct ParentOrParentsExecutivePlurality;
impl Contains<Location> for ParentOrParentsExecutivePlurality {
    fn contains(location: &Location) -> bool {
        matches!(location.unpack(), (1, []) | (1, [Plurality { .. }]))
    }
}

/// Filter that matches any sibling parachain origin.
pub struct IsSiblingParachain;
impl Contains<Location> for IsSiblingParachain {
    fn contains(location: &Location) -> bool {
        matches!(location.unpack(), (1, [Parachain(_)]))
    }
}

pub struct FellowsPlurality;
impl Contains<Location> for FellowsPlurality {
    fn contains(location: &Location) -> bool {
        matches!(
            location.unpack(),
            (
                1,
                [
                    Parachain(COLLECTIVES_ID),
                    Plurality {
                        id: BodyId::Technical,
                        ..
                    }
                ]
            )
        )
    }
}

pub type Barrier = TrailingSetTopicAsId<(
    TakeWeightCredit,
    AllowKnownQueryResponses<PolkadotXcm>,
    WithComputedOrigin<
        (
            AllowTopLevelPaidExecutionFrom<Everything>,
            AllowExplicitUnpaidExecutionFrom<(
                ParentOrParentsExecutivePlurality,
                FellowsPlurality,
                Equals<GovernanceLocation>,
                IsSiblingParachain,
            )>,
            AllowSubscriptionsFrom<ParentRelayOrSiblingParachains>,
            AllowHrmpNotificationsFromRelayChain,
        ),
        UniversalLocation,
        ConstU32<8>,
    >,
)>;

/// Locations that will not be charged fees in the executor, neither for execution nor delivery.
/// We only waive fees for system functions, which these locations represent.
pub type WaivedLocations = (
    Equals<RootLocation>,
    RelayOrOtherSystemParachains<AllSiblingSystemParachains, Runtime>,
    Equals<AssetHubLocation>,
);

/// Cases where a remote origin is accepted as trusted Teleporter for a given asset.
/// Trust the relay chain and other system parachains to teleport the relay chain native token.
pub type TrustedTeleporters = ConcreteAssetFromSystem<RelayLocation>;

/// Defines origin aliasing rules for this chain.
pub type TrustedAliasers = (
    AliasChildLocation,
    AliasAccountId32FromSiblingSystemChain,
    AliasOriginRootUsingFilter<AssetHubLocation, Everything>,
    AuthorizedAliasers<Runtime>,
);

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
    type RuntimeCall = RuntimeCall;
    type XcmSender = XcmRouter;
    type AssetTransactor = LocalAssetTransactor;
    type OriginConverter = XcmOriginToTransactDispatchOrigin;
    type IsReserve = ();
    type IsTeleporter = TrustedTeleporters;
    type UniversalLocation = UniversalLocation;
    type Barrier = Barrier;
    type Weigher = WeightInfoBounds<
        crate::weights::xcm::Web3StoragePaseoXcmWeight<RuntimeCall>,
        RuntimeCall,
        MaxInstructions,
    >;
    type Trader = UsingComponents<
        WeightToFee,
        RelayLocation,
        AccountId,
        Balances,
        ResolveTo<StakingPotAccountId<Runtime>, Balances>,
    >;
    type ResponseHandler = PolkadotXcm;
    type AssetTrap = PolkadotXcm;
    type SubscriptionService = PolkadotXcm;
    type PalletInstancesInfo = AllPalletsWithSystem;
    type MaxAssetsIntoHolding = MaxAssetsIntoHolding;
    type AssetLocker = ();
    type AssetExchanger = ();
    type FeeManager = XcmFeeManagerFromComponents<
        WaivedLocations,
        SendXcmFeeToAccount<LocalAssetTransactor, TreasuryLocation>,
    >;
    type MessageExporter = ();
    type UniversalAliases = Nothing;
    type CallDispatcher = RuntimeCall;
    type SafeCallFilter = Everything;
    type Aliasers = TrustedAliasers;
    type TransactionalProcessor = FrameTransactionalProcessor;
    type HrmpNewChannelOpenRequestHandler = ();
    type HrmpChannelAcceptedHandler = ();
    type HrmpChannelClosingHandler = ();
    type XcmRecorder = PolkadotXcm;
    type XcmEventEmitter = PolkadotXcm;
}

parameter_types! {
    pub const UnitWeightCost: Weight = Weight::from_parts(1_000_000_000, 64 * 1024);
}

/// Price for delivering XCM to the relay chain via UMP.
pub type PriceForParentDelivery =
    ExponentialPrice<FeeAssetId, BaseDeliveryFee, TransactionByteFee, ParachainSystem>;

/// The means for routing XCM messages which are not for local execution into the right message
/// queues.
pub type XcmRouter = WithUniqueTopic<(
    // Two routers - use UMP to communicate with the relay chain:
    cumulus_primitives_utility::ParentAsUmp<ParachainSystem, PolkadotXcm, PriceForParentDelivery>,
    // ..and XCMP to communicate with the sibling chains.
    XcmpQueue,
)>;

/// Local origins on this chain are allowed to dispatch XCM sends/executions.
pub type LocalOriginToLocation = SignedToAccountId32<RuntimeOrigin, AccountId, RelayNetwork>;

impl pallet_xcm::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    // Disallow users sending arbitrary XCM programs from this chain.
    type SendXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, ()>;
    type XcmRouter = XcmRouter;
    type ExecuteXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
    type XcmExecuteFilter = Everything;
    type XcmExecutor = XcmExecutor<XcmConfig>;
    type XcmTeleportFilter = Everything;
    type XcmReserveTransferFilter = Nothing;
    type Weigher = WeightInfoBounds<
        crate::weights::xcm::Web3StoragePaseoXcmWeight<RuntimeCall>,
        RuntimeCall,
        MaxInstructions,
    >;
    type UniversalLocation = UniversalLocation;
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeCall = RuntimeCall;
    const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
    type AdvertisedXcmVersion = pallet_xcm::CurrentXcmVersion;
    type Currency = Balances;
    type CurrencyMatcher = ();
    type TrustedLockers = ();
    type SovereignAccountOf = LocationToAccountId;
    type MaxLockers = ConstU32<8>;
    type WeightInfo = crate::weights::pallet_xcm::WeightInfo<Runtime>;
    type AdminOrigin = EnsureRoot<AccountId>;
    type MaxRemoteLockConsumers = ConstU32<0>;
    type RemoteLockConsumerIdentifier = ();
    type AuthorizedAliasConsideration = HoldConsideration<
        AccountId,
        Balances,
        AuthorizeAliasHoldReason,
        LinearStoragePrice<DepositPerItem, DepositPerByte, Balance>,
    >;
}

impl cumulus_pallet_xcm::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type XcmExecutor = XcmExecutor<XcmConfig>;
}
