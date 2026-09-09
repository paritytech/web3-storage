// SPDX-License-Identifier: Apache-2.0

//! The deterministic finding scenarios of `specs/quint/challenges_test.qnt`
//! (module `challengesCodeTest`), replayed step-for-step against the real
//! pallet through the MBT driver. `quint test` traces carry no
//! `mbt::actionTaken` metadata, so these are hand-mirrored rather than
//! replayed via `#[quint_test]` — keep them in lockstep with the .qnt file.
//!
//! Like their Quint counterparts, the finding tests assert the current,
//! BROKEN behaviour on purpose: they go red the day the pallet closes the
//! hole, signalling that spec, driver and scenario must move together.
//!
//! These run under plain `cargo test` (no quint CLI needed); the random
//! trace replay lives in `tests/mbt.rs` behind the `mbt` feature.

use pallet_storage_provider_mbt::{Acct, ChallengeDriver, CommitmentQ, Rsp, SpecState};
use quint_connect::State as _;

const C_A: CommitmentQ = CommitmentQ {
    root: 1,
    start: 0,
    count: 2,
};
const C_B: CommitmentQ = CommitmentQ {
    root: 2,
    start: 0,
    count: 3,
};

fn init() -> ChallengeDriver {
    let mut d = ChallengeDriver::default();
    d.init_chain().unwrap();
    d
}

fn state(d: &ChallengeDriver) -> SpecState {
    SpecState::from_driver(d).unwrap()
}

/// The spec's init state must be reproduced exactly by the setup phase.
#[test]
fn init_matches_spec() {
    let d = init();
    let s = state(&d);
    assert_eq!(s.now, 1);
    for p in [Acct::P1, Acct::P2] {
        assert!(s.prov[&p].registered);
        assert_eq!(s.prov[&p].stake, 100);
        assert_eq!(s.prov[&p].deregister_at, -1);
        assert_eq!(s.prov[&p].pending, 0);
        assert_eq!(s.free[&p], 20);
        assert_eq!(s.reserved[&p], 100);
    }
    assert_eq!(s.free[&Acct::Adm], 100);
    assert_eq!(s.free[&Acct::Pub], 100);
    assert_eq!(s.free[&Acct::Tre], 0);
    assert_eq!(s.reserved[&Acct::Adm], 0);
    assert_eq!(s.canonical.c, pallet_storage_provider_mbt::NO_COMMIT);
    assert!(s.open.is_empty());
}

/// Finding 1 (`anyLeafProofEscapeTest`): the provider lost the challenged
/// leaf 1 but still holds leaf 0 under the same root; because
/// `verify_mmr_proof` never checks the proven leaf's position against
/// `challenge.target.leaf_index`, the stale leaf defends the challenge.
#[test]
fn any_leaf_proof_escape() {
    let mut d = init();
    d.sign_offchain(C_A).unwrap();
    d.do_checkpoint(C_A).unwrap();
    d.lose_leaf(Acct::P1, 1).unwrap();
    d.challenge_checkpoint(Acct::Pub, 1).unwrap();
    d.respond((4, 0), Rsp::RProof).unwrap();

    let s = state(&d);
    assert!(s.open.is_empty(), "challenge resolved");
    // Defended, not full-stake slashed: only the 10% defense cost-split
    // left the stake (finding 3), the Timeout/InvalidProof path would have
    // zeroed it.
    assert_eq!(s.prov[&Acct::P1].stake, 99);
}

/// Finding 2 (`supersededEscapeTest`): a replica whose last_sync (cA) lags
/// canonical (cB) drops ALL its data yet defends any challenge on the
/// canonical range with `Superseded`.
#[test]
fn superseded_escape() {
    let mut d = init();
    d.sign_offchain(C_A).unwrap();
    d.do_checkpoint(C_A).unwrap();
    d.replica_sync(C_A).unwrap();
    d.sign_offchain(C_B).unwrap();
    d.do_checkpoint(C_B).unwrap();
    d.lose_leaf(Acct::P2, 0).unwrap();
    d.lose_leaf(Acct::P2, 1).unwrap();
    d.challenge_replica(Acct::Adm, 0).unwrap();
    d.respond((4, 0), Rsp::RSuperseded).unwrap();

    let s = state(&d);
    assert!(s.open.is_empty());
    assert_eq!(
        s.prov[&Acct::P2].stake,
        99,
        "defended while holding nothing"
    );
}

/// Finding 3 (`defenseGrindsStakeTest`): a correct, immediate defense
/// against a public stranger still slashes 10% of the deposit from provider
/// stake into the Treasury — the tier split of the design is absent.
#[test]
fn defense_grinds_stake() {
    let mut d = init();
    d.sign_offchain(C_A).unwrap();
    d.do_checkpoint(C_A).unwrap();
    d.challenge_checkpoint(Acct::Pub, 0).unwrap();
    d.respond((4, 0), Rsp::RProof).unwrap();

    let s = state(&d);
    assert_eq!(s.prov[&Acct::P1].stake, 100 - 1);
    assert_eq!(s.free[&Acct::Tre], 1);
    assert_eq!(
        s.free[&Acct::Pub],
        100 - 9,
        "stranger pays 90% of the deposit"
    );
    assert_eq!(s.free[&Acct::P1], 20 + 9, "... straight to the provider");
}

/// Finding 4, code semantics (`outOfRangeMaskedTest`): a challenge on
/// nonexistent leaf 4 of a 2-leaf commitment is accepted on-chain, and the
/// unbound proof "defends" it — two bugs cancelling.
#[test]
fn out_of_range_masked() {
    let mut d = init();
    d.sign_offchain(C_A).unwrap();
    d.do_checkpoint(C_A).unwrap();
    d.challenge_checkpoint(Acct::Pub, 4).unwrap();
    d.respond((4, 0), Rsp::RProof).unwrap();

    let s = state(&d);
    assert!(s.open.is_empty());
    assert_eq!(
        s.prov[&Acct::P1].stake,
        99,
        "honest provider defended out-of-range challenge"
    );
}

/// Green path (`timeoutSlashTest`): an unanswered challenge times out at
/// the sweep, the full stake goes to the Treasury, the deposit comes back.
#[test]
fn timeout_slash() {
    let mut d = init();
    d.sign_offchain(C_A).unwrap();
    d.do_checkpoint(C_A).unwrap();
    d.challenge_checkpoint(Acct::Adm, 0).unwrap();
    d.advance(2).unwrap();
    d.advance(2).unwrap();

    let s = state(&d);
    assert!(s.open.is_empty());
    assert_eq!(s.prov[&Acct::P1].stake, 0);
    assert_eq!(s.prov[&Acct::P1].pending, 0);
    assert_eq!(s.free[&Acct::Adm], 100, "deposit refunded, no reward");
    assert_eq!(s.free[&Acct::Tre], 100, "full stake to the Treasury");
    assert_eq!(s.reserved[&Acct::P1], 0);
}

/// Green path (`deletedDefenseTest`): an admin-signed deletion past the
/// challenged seq is a legitimate defense. Note this only verifies because
/// the driver registers the admin as a zero-stake provider — on the real
/// chain `verify_signature` cannot resolve a non-provider admin's key
/// (finding, by inspection).
#[test]
fn deleted_defense() {
    let mut d = init();
    d.sign_offchain(C_A).unwrap();
    d.do_checkpoint(C_A).unwrap();
    d.challenge_checkpoint(Acct::Adm, 0).unwrap();
    d.admin_delete_to(2).unwrap();
    d.respond((4, 0), Rsp::RDeleted).unwrap();

    let s = state(&d);
    assert!(s.open.is_empty());
    assert!(s.prov[&Acct::P1].stake > 0, "defended");
}

/// Green path (`deregisterTest`): full deregistration only after expiry,
/// agreement teardown, and the announcement window.
#[test]
fn deregister() {
    let mut d = init();
    for _ in 0..4 {
        d.advance(2).unwrap(); // now 9 > AGREEMENT_END
    }
    d.announce_dereg(Acct::P1).unwrap();
    d.advance(2).unwrap();
    d.advance(2).unwrap(); // now 13 >= deregisterAt
    d.complete_dereg(Acct::P1).unwrap();

    let s = state(&d);
    assert!(!s.prov[&Acct::P1].registered);
    assert_eq!(s.prov[&Acct::P1].deregister_at, 13);
    assert_eq!(s.free[&Acct::P1], 20 + 100);
    assert_eq!(s.reserved[&Acct::P1], 0);
}
