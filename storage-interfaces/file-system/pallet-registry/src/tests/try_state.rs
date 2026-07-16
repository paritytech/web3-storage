// SPDX-License-Identifier: Apache-2.0

//! `try_state` invariant checks exercised against real post-extrinsic state:
//! the invariants hold on state the pallet's own extrinsics produce, and a
//! deliberate corruption is detected.

use super::*;

#[test]
fn try_state_holds_and_detects_corruption() {
    new_test_ext().execute_with(|| {
        advance_to_block_1();

        let (provider_pk, provider) = setup_provider();
        let terms = primary_terms(1, 100, 500, 1, 100);
        let sig = sign_terms(&provider_pk, &terms);

        assert_ok!(DriveRegistry::create_drive(
            RuntimeOrigin::signed(1),
            None,
            provider,
            terms,
            sig
        ));

        // Index invariants hold on real state.
        assert_ok!(DriveRegistry::do_try_state());

        // UserDrives now references a non-existent drive.
        crate::UserDrives::<Test>::mutate(1u64, |ds| {
            ds.try_push(999).expect("bound not reached");
        });
        assert!(DriveRegistry::do_try_state().is_err());
    });
}
