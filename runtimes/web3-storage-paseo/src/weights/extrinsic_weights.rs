// SPDX-License-Identifier: GPL-3.0-only

//! Extrinsic weight constants for the runtime.

pub mod constants {
    use frame_support::{
        parameter_types,
        weights::{constants, Weight},
    };

    parameter_types! {
        /// Executing a NO-OP extrinsic takes ~94.000 ns
        /// (on reference hardware), which provides a baseline for extrinsic overhead.
        pub const ExtrinsicBaseWeight: Weight = Weight::from_parts(
            constants::WEIGHT_REF_TIME_PER_NANOS.saturating_mul(94_000),
            0,
        );
    }

    #[cfg(test)]
    mod test_weights {
        use frame_support::weights::constants;

        /// Checks that the weight exists and is sane.
        #[test]
        fn sane() {
            let w = super::constants::ExtrinsicBaseWeight::get();
            assert!(w.ref_time() >= 10u64.saturating_mul(constants::WEIGHT_REF_TIME_PER_NANOS));
            assert!(w.ref_time() <= constants::WEIGHT_REF_TIME_PER_MILLIS);
        }
    }
}
