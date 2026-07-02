// SPDX-License-Identifier: Apache-2.0

//! Block weight constants for the runtime.

pub mod constants {
    use frame_support::{
        parameter_types,
        weights::{constants, Weight},
    };

    parameter_types! {
        /// Executing a NO-OP `System::remarks` block takes ~175.000 ns
        /// (on reference hardware), which sets a baseline for block execution.
        pub const BlockExecutionWeight: Weight = Weight::from_parts(
            constants::WEIGHT_REF_TIME_PER_NANOS.saturating_mul(175_000),
            0,
        );
    }

    #[cfg(test)]
    mod test_weights {
        use frame_support::weights::constants;

        /// Checks that the weight exists and is same.
        #[test]
        fn sane() {
            let w = super::constants::BlockExecutionWeight::get();
            assert!(w.ref_time() >= 100u64.saturating_mul(constants::WEIGHT_REF_TIME_PER_NANOS));
            assert!(w.ref_time() <= 50u64.saturating_mul(constants::WEIGHT_REF_TIME_PER_MILLIS));
        }
    }
}
