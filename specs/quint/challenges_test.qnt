// Deterministic scenarios for challenges.qnt (issue #265).
//
// Each *EscapeTest / *GriefingTest / *GrindsStakeTest run is a minimal
// counterexample to a design guarantee — kept green ON PURPOSE: they assert
// the current (broken) behavior, so they will FAIL the day the pallet or the
// design closes the hole, flagging that this spec must be updated.

// ── code semantics (verify_mmr_proof unbound to leaf_index) ──────────────
module challengesCodeTest {
  import challenges(
    CHALLENGE_TIMEOUT = 3,
    DEREG_PERIOD = 4,
    DEPOSIT = 10,
    STAKE = 100,
    MAX_PER_DEADLINE = 2,
    AGREEMENT_END = 8,
    STRICT_PROOF = false
  ).* from "./challenges"

  // Config invariants the runtime's integrity_test enforces.
  run integrityConstantsTest = all {
    DEREG_PERIOD > CHALLENGE_TIMEOUT,
    DEPOSIT > 0,
    STAKE > 0,
    MAX_PER_DEADLINE > 0,
  }

  // FINDING (dishonestAlwaysSlashable broken): the provider lost the
  // challenged leaf 1 but still holds leaf 0 under the same root; because
  // verify_mmr_proof never checks the proven leaf's position against
  // challenge.target.leaf_index, the stale leaf defends the challenge.
  run anyLeafProofEscapeTest =
    init
      .then(signOffchainC(cA))
      .then(doCheckpointC(cA))
      .then(loseLeaf(P1, 1))
      .then(challengeCheckpointAs(Pub, 1))
      .then(respondAs((4, 0), RProof))
      .expect(and {
        log.length() == 1,
        log.nth(0).outcome == DefendedProof,
        not(log.nth(0).heldChallenged),
        prov.get(P1).stake > 0,
      })

  // FINDING (dishonestAlwaysSlashable broken): a replica whose last_sync (cA)
  // is behind canonical (cB) drops ALL data, yet defends any challenge on the
  // canonical range with Superseded — root differs from canonical and the
  // challenged seq is inside the canonical range. It can repeat this for
  // every challenge until it re-syncs, so its data is unrecoverable and its
  // stake untouchable. Present in the design text too (implementation.md
  // "Superseded" allows challenged_seq < canonical_end).
  run supersededEscapeTest =
    init
      .then(signOffchainC(cA))
      .then(replicaSyncC(cA))
      .then(signOffchainC(cB))
      .then(doCheckpointC(cB))
      .then(loseLeaf(P2, 0))
      .then(loseLeaf(P2, 1))
      .then(challengeReplicaAs(Adm, 0))
      .then(respondAs((4, 0), RSuperseded))
      .expect(and {
        log.length() == 1,
        log.nth(0).outcome == DefendedSuperseded,
        not(log.nth(0).heldChallenged),
        not(log.nth(0).legitDeleted),
        prov.get(P2).stake > 0,
      })

  // FINDING (design: "stake is never touched on a valid response"): a public
  // stranger challenges, the provider defends immediately and correctly, yet
  // 10% of the deposit is slashed from provider stake into the Treasury and
  // the stranger only pays 90%. Repeatable — a crowd of strangers grinds an
  // honest provider's stake down, the exact anti-DDoS drain the design closes
  // by making the public tier pay 100% (tier logic is absent from the code).
  run defenseGrindsStakeTest =
    init
      .then(signOffchainC(cA))
      .then(doCheckpointC(cA))
      .then(challengeCheckpointAs(Pub, 0))
      .then(respondAs((4, 0), RProof))
      .expect(and {
        log.nth(0).outcome == DefendedProof,
        prov.get(P1).stake == STAKE - 1,      // 10% of DEPOSIT slashed from stake
        free.get(Tre) == 1,                   // ... into the Treasury
        free.get(Pub) == 100 - 9,             // stranger pays 90% of the deposit
        free.get(P1) == 20 + 9,               // ... straight to the provider
      })

  // FINDING (missing bounds check, masked): no extrinsic validates
  // target.leaf_index < leaf_count, so a challenge on nonexistent leaf 4 of a
  // 2-leaf commitment is accepted — and under code semantics the fully honest
  // provider "defends" it via the unbound proof. Two bugs cancelling.
  run outOfRangeMaskedTest =
    init
      .then(signOffchainC(cA))
      .then(doCheckpointC(cA))
      .then(challengeCheckpointAs(Pub, 4))
      .then(respondAs((4, 0), RProof))
      .expect(and {
        not(log.nth(0).inRange),
        log.nth(0).outcome == DefendedProof,
      })

  // Green path: unanswered challenge times out at the sweep, full stake goes
  // to the Treasury, the challenger's deposit comes back, money conserved.
  run timeoutSlashTest =
    init
      .then(signOffchainC(cA))
      .then(doCheckpointC(cA))
      .then(challengeCheckpointAs(Adm, 0))
      .then(advanceBlockBy(2))
      .then(advanceBlockBy(2))
      .expect(and {
        log.nth(0).outcome == TimeoutSlash,
        prov.get(P1).stake == 0,
        free.get(Adm) == 100,
        free.get(Tre) == STAKE,
        moneyConserved,
        reservedAccounted,
      })

  // Green path: admin-signed deletion is a legitimate defense.
  run deletedDefenseTest =
    init
      .then(signOffchainC(cA))
      .then(doCheckpointC(cA))
      .then(challengeCheckpointAs(Adm, 0))
      .then(adminDeleteTo(2))
      .then(respondAs((4, 0), RDeleted))
      .expect(and {
        log.nth(0).outcome == DefendedDeleted,
        log.nth(0).legitDeleted,
      })

  // Green path: full deregistration only after challenges resolve and the
  // announcement window elapses.
  run deregisterTest =
    init
      .then(advanceBlockBy(2))   // now 3
      .then(advanceBlockBy(2))   // now 5
      .then(advanceBlockBy(2))   // now 7
      .then(advanceBlockBy(2))   // now 9 >= AGREEMENT_END
      .then(announceDeregP(P1))  // deregisterAt = 13
      .then(advanceBlockBy(2))   // now 11
      .then(advanceBlockBy(2))   // now 13
      .then(completeDeregP(P1))
      .expect(and {
        not(prov.get(P1).registered),
        free.get(P1) == 20 + STAKE,
        reserved.get(P1) == 0,
        moneyConserved,
      })
}

// ── design semantics (proof bound to the challenged leaf) ────────────────
module challengesDesignTest {
  import challenges(
    CHALLENGE_TIMEOUT = 3,
    DEREG_PERIOD = 4,
    DEPOSIT = 10,
    STAKE = 100,
    MAX_PER_DEADLINE = 2,
    AGREEMENT_END = 8,
    STRICT_PROOF = true
  ).* from "./challenges"

  // FINDING (honestNeverSlashed broken at the design level): once the proof
  // is position-bound as the design specifies, the missing leaf_index bounds
  // check at challenge creation becomes fatal — a challenge on nonexistent
  // leaf 4 of a 2-leaf commitment has NO valid defense (no proof exists,
  // Superseded fails since the seq is outside canonical, Deleted needs an
  // admin signature), so a fully honest provider holding every byte is
  // full-stake slashed by any stranger for one deposit.
  run outOfRangeGriefingTest =
    init
      .then(signOffchainC(cA))
      .then(doCheckpointC(cA))
      .then(challengeCheckpointAs(Pub, 4))
      .then(respondAs((4, 0), RProof))
      .expect(and {
        log.nth(0).outcome == InvalidProofSlash,
        log.nth(0).heldAllUnderRoot,          // provider held everything
        prov.get(P1).stake == 0,              // and lost its whole stake
      })

  // The Superseded escape is independent of proof binding — it survives the
  // design-semantics fix too (the hole is in the Superseded rule itself).
  run supersededEscapeStillTest =
    init
      .then(signOffchainC(cA))
      .then(replicaSyncC(cA))
      .then(signOffchainC(cB))
      .then(doCheckpointC(cB))
      .then(loseLeaf(P2, 0))
      .then(loseLeaf(P2, 1))
      .then(challengeReplicaAs(Adm, 0))
      .then(respondAs((4, 0), RSuperseded))
      .expect(and {
        log.nth(0).outcome == DefendedSuperseded,
        not(log.nth(0).heldChallenged),
        prov.get(P2).stake > 0,
      })
}
