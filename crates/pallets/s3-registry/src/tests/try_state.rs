// SPDX-License-Identifier: Apache-2.0

//! `try_state` invariant checks exercised against real post-extrinsic state:
//! the invariants hold on state the pallet's own extrinsics produce, and a
//! deliberate corruption is detected.

use super::*;

#[test]
fn try_state_holds_and_detects_corruption() {
    new_test_ext().execute_with(|| {
        let s3_bucket_id = setup_provider_and_s3_bucket(1, 1);
        let cid = sp_core::H256::repeat_byte(0xAB);
        assert_ok!(S3Registry::put_object_metadata(
            RuntimeOrigin::signed(1),
            s3_bucket_id,
            b"photos/cat.jpg".to_vec(),
            cid,
            1024,
            b"image/jpeg".to_vec(),
            vec![],
        ));

        // Index and counter invariants hold on real state.
        assert_ok!(S3Registry::do_try_state());

        // object_count no longer matches the actual Objects entries.
        S3Buckets::<Test>::mutate(s3_bucket_id, |b| {
            b.as_mut().unwrap().object_count += 1;
        });
        assert!(S3Registry::do_try_state().is_err());

        // Restore object_count, then corrupt total_size instead.
        S3Buckets::<Test>::mutate(s3_bucket_id, |b| {
            b.as_mut().unwrap().object_count -= 1;
        });
        assert_ok!(S3Registry::do_try_state());
        S3Buckets::<Test>::mutate(s3_bucket_id, |b| {
            b.as_mut().unwrap().total_size += 1;
        });
        assert!(S3Registry::do_try_state().is_err());
    });
}
