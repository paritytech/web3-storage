// SPDX-License-Identifier: GPL-3.0-only

//! Web3 Storage Paseo Parachain runtime constants used by this runtime.

/// Inlined subset of the Paseo relay chain runtime constants crate.
mod paseo_runtime_constants {
    pub mod system_parachain {
        pub const ASSET_HUB_ID: u32 = 1000;
        pub const COLLECTIVES_ID: u32 = 1001;
        pub const PEOPLE_ID: u32 = 1004;
    }
}

/// System parachain ids on Paseo.
pub use paseo_runtime_constants::system_parachain;

/// Consensus-related.
pub mod consensus {
    use frame_support::weights::{constants::WEIGHT_REF_TIME_PER_SECOND, Weight};

    /// How many parachain blocks are processed by the relay chain per parent. With 3 cores
    /// assigned to the parachain and 2 s parachain blocks against a 6 s relay slot, each relay
    /// slot includes 3 parachain candidates.
    pub const BLOCK_PROCESSING_VELOCITY: u32 = 3;
    /// Relay chain slot duration, in milliseconds. The relay chain is independent of the
    /// parachain block time and continues to produce a slot every 6 s.
    pub const RELAY_CHAIN_SLOT_DURATION_MILLIS: u32 = 6000;

    /// Average expected block time targeted by the parachain. Picked up by `pallet_timestamp` and
    /// `pallet_aura`.
    pub const MILLISECS_PER_BLOCK: u64 = 2000;

    /// Slot duration equals block time for this runtime.
    pub const SLOT_DURATION: u64 = MILLISECS_PER_BLOCK;

    /// Required by slot-based authoring with elastic scaling.
    pub const RELAY_PARENT_OFFSET: u32 = 1;

    /// 2 seconds of compute per parachain block (one relay-chain core).
    pub const MAXIMUM_BLOCK_WEIGHT: Weight = Weight::from_parts(
        WEIGHT_REF_TIME_PER_SECOND.saturating_mul(2),
        cumulus_primitives_core::relay_chain::MAX_POV_SIZE as u64,
    );

    /// Parameters enabling async backing functionality.
    pub mod async_backing {
        use super::{BLOCK_PROCESSING_VELOCITY, RELAY_PARENT_OFFSET};

        /// Maximum number of blocks simultaneously accepted by the Runtime, not yet included
        /// into the relay chain.
        ///
        /// Sized in relay slots, then converted to parachain blocks. A candidate takes ~2 relay
        /// slots to be backed and included under async backing; `RELAY_PARENT_OFFSET` adds the
        /// extra relay parents the offset lets us build against, and `+ 1` is the in-flight slot.
        /// Multiplying by `BLOCK_PROCESSING_VELOCITY` turns relay slots into parachain blocks.
        /// With offset 1 and velocity 3 this is `(3 + 1) * 3 = 12`.
        pub const UNINCLUDED_SEGMENT_CAPACITY: u32 =
            (RELAY_SLOTS_OF_CAPACITY + RELAY_PARENT_OFFSET) * BLOCK_PROCESSING_VELOCITY;

        /// Relay slots of unincluded data to buffer, before accounting for the relay-parent
        /// offset: 2 slots for the backing+inclusion pipeline plus 1 in-flight slot.
        const RELAY_SLOTS_OF_CAPACITY: u32 = 3;
    }
}

/// Time-related.
pub mod time {
    use crate::BlockNumber;

    pub const MINUTES: BlockNumber =
        60_000 / (super::consensus::MILLISECS_PER_BLOCK as BlockNumber);
    pub const HOURS: BlockNumber = MINUTES * 60;
}

/// Durations measured in RELAY chain blocks (6s), independent of the
/// parachain block time. All storage-pallet timeouts are denominated in relay
/// blocks (via `pallet_storage_provider::Config::BlockNumberProvider`) so
/// they keep their wall-clock meaning when the parachain block time changes.
pub mod relay_time {
    use crate::BlockNumber;

    pub const MINUTES: BlockNumber =
        60_000 / (super::consensus::RELAY_CHAIN_SLOT_DURATION_MILLIS as BlockNumber);
    pub const HOURS: BlockNumber = MINUTES * 60;
}

/// Constants relating to the native token.
pub mod currency {
    use crate::Balance;

    /// 1 token with 12 decimal places (like Polkadot).
    pub const UNIT: Balance = 1_000_000_000_000;
    pub const MILLIUNIT: Balance = 1_000_000_000;
    pub const MICROUNIT: Balance = 1_000_000;

    /// The existential deposit: set to 1/10 of the Connected Relay Chain.
    pub const EXISTENTIAL_DEPOSIT: Balance = MILLIUNIT;
}

/// Block weight dispatch-class ratios.
pub mod system {
    use sp_runtime::Perbill;

    /// ~5% of the block weight is consumed by `on_initialize` handlers. Used to limit the
    /// maximal weight of a single extrinsic.
    pub const AVERAGE_ON_INITIALIZE_RATIO: Perbill = Perbill::from_percent(5);

    /// `Normal` extrinsics fill up to 75% of the block; the rest is reserved for `Operational`.
    pub const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);
}

/// Fee-related constants.
pub mod fee {
    use crate::Balance;

    /// Weight fee multiplier: 1 unit of balance per unit of ref_time.
    pub const WEIGHT_FEE: Balance = 1;
}

/// Well-known XCM locations on Paseo.
pub mod locations {
    use frame_support::parameter_types;
    use xcm::latest::prelude::{Junction::*, Location};

    use super::paseo_runtime_constants;

    parameter_types! {
        pub AssetHubLocation: Location =
            Location::new(1, Parachain(paseo_runtime_constants::system_parachain::ASSET_HUB_ID));
        pub PeopleLocation: Location =
            Location::new(1, Parachain(paseo_runtime_constants::system_parachain::PEOPLE_ID));
        /// Governance is conducted on Asset Hub for Paseo.
        pub GovernanceLocation: Location =
            Location::new(1, Parachain(paseo_runtime_constants::system_parachain::ASSET_HUB_ID));
    }
}

/// Default XCM version for genesis config.
pub mod xcm_version {
    pub const SAFE_XCM_VERSION: u32 = xcm::prelude::XCM_VERSION;
}
