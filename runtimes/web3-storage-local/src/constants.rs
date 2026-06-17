// SPDX-License-Identifier: GPL-3.0-only

//! Web3 Storage Parachain runtime constants used by this runtime.

/// System parachain IDs (standard across Polkadot/Paseo system chains).
pub mod system_parachain {
    pub const ASSET_HUB_ID: u32 = 1000;
    pub const COLLECTIVES_ID: u32 = 1001;
    pub const PEOPLE_ID: u32 = 1004;
}

/// Consensus-related.
pub mod consensus {
    use frame_support::weights::{constants::WEIGHT_REF_TIME_PER_SECOND, Weight};

    /// How many parachain blocks are processed by the relay chain per parent. Limits the
    /// number of blocks authored per slot.
    pub const BLOCK_PROCESSING_VELOCITY: u32 = 1;
    /// Relay chain slot duration, in milliseconds.
    pub const RELAY_CHAIN_SLOT_DURATION_MILLIS: u32 = 6000;

    /// Average expected block time targeted by the parachain. Picked up by `pallet_timestamp` and
    /// `pallet_aura`.
    pub const MILLISECS_PER_BLOCK: u64 = 6000;

    /// Slot duration equals block time for this runtime.
    pub const SLOT_DURATION: u64 = MILLISECS_PER_BLOCK;

    /// 2 seconds of compute with a 6 second average block.
    pub const MAXIMUM_BLOCK_WEIGHT: Weight = Weight::from_parts(
        WEIGHT_REF_TIME_PER_SECOND.saturating_mul(2),
        cumulus_primitives_core::relay_chain::MAX_POV_SIZE as u64,
    );

    /// Parameters enabling async backing functionality.
    pub mod async_backing {
        /// Maximum number of blocks simultaneously accepted by the Runtime, not yet included
        /// into the relay chain.
        pub const UNINCLUDED_SEGMENT_CAPACITY: u32 = 3;
    }
}

/// Time-related.
pub mod time {
    use crate::BlockNumber;

    pub const MINUTES: BlockNumber =
        60_000 / (super::consensus::MILLISECS_PER_BLOCK as BlockNumber);
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

/// Well-known XCM locations.
pub mod locations {
    use frame_support::parameter_types;
    use xcm::latest::prelude::{Junction::*, Location};

    use super::system_parachain;

    parameter_types! {
        pub AssetHubLocation: Location =
            Location::new(1, Parachain(system_parachain::ASSET_HUB_ID));
        pub PeopleLocation: Location =
            Location::new(1, Parachain(system_parachain::PEOPLE_ID));
        /// Governance is conducted on Asset Hub.
        pub GovernanceLocation: Location =
            Location::new(1, Parachain(system_parachain::ASSET_HUB_ID));
    }
}

/// Default XCM version for genesis config.
pub mod xcm_version {
    pub const SAFE_XCM_VERSION: u32 = xcm::prelude::XCM_VERSION;
}
