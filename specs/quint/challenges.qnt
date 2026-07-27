// Quint specification of the challenge protocol (issue #265).
//
// WHAT IT MODELS (code semantics, so traces are replayable against the pallet):
//   crates/pallets/storage-provider/src/impls/challenges.rs   — create/sweep/slash
//   crates/pallets/storage-provider/src/lib.rs                — challenge_checkpoint,
//     challenge_offchain, challenge_replica, respond_to_challenge, deregister flow
//   crates/primitives/storage/src/lib.rs                      — Commitment ranges,
//     verify_mmr_proof / verify_merkle_proof (abstracted, see STRICT_PROOF)
//
// INVARIANTS encode the guarantees of docs/design/scalable-web3-storage.md
// ("The Challenge Game") and docs/design/scalable-web3-storage-implementation.md
// ("Challenge Protocol"). Where code and design disagree, an invariant breaks
// and the counterexample trace is the finding.
//
// STRICT_PROOF selects the proof-verification semantics:
//   false — as implemented: verify_mmr_proof(proof, root) does NOT bind the
//           proof to challenge.target.leaf_index; any leaf the provider still
//           holds under the challenged root yields a valid response.
//   true  — as designed (implementation.md "Verification" passes
//           challenge.leaf_index into verify_mmr_proof): only the challenged
//           leaf itself defends.
//
// ABSTRACTIONS (what, not how):
//   - Crypto is a ghost: `leaves` is the set of sequence numbers a provider
//     physically holds; proofs/signatures verify iff the corresponding ghost
//     state supports them. MMR/Merkle internals are out of scope.
//   - One bucket; P1 is its only primary, P2 its only replica; Adm is the
//     bucket admin & agreement owner (authorized tier), Pub a stranger.
//   - Both agreements exist from init and expire at AGREEMENT_END; settlement
//     and payment streams are out of scope (challenge paths only check
//     `now < expires_at`, which is modeled).
//   - The anchor clock jumps 1..2 relay blocks per step; the on_initialize
//     sweep's one-block lag and PoV budgets (MAX_SWEEP_SPAN/SLASH_BUDGET) are
//     collapsed into "advancing past the deadline sweeps it". The lag only
//     delays slashing/counter-decrements, which is the safe direction for
//     every invariant here.
//   - Challenger tx fees are out of scope; only deposit, stake, and treasury
//     flows are tracked (they are what conservation is about).
//   - No cancel action: the design specifies challenger cancellation, the
//     pallet does not implement it (finding, by inspection).
//   - Providers do not challenge themselves (the pallet allows it; it only
//     aliases the reserved pot without affecting these invariants).
module challenges {
  // ── constants (runtime Config) ─────────────────────────────────────────
  const CHALLENGE_TIMEOUT: int   // ChallengeTimeout, anchor blocks
  const DEREG_PERIOD: int        // DeregisterAnnouncementPeriod, > CHALLENGE_TIMEOUT
  const DEPOSIT: int             // ChallengeDeposit
  const STAKE: int               // provider stake locked at registration
  const MAX_PER_DEADLINE: int    // MaxChallengesPerDeadline
  const AGREEMENT_END: int       // expires_at of both agreements
  const STRICT_PROOF: bool       // see header

  // ── domains ────────────────────────────────────────────────────────────
  type Acct = P1 | P2 | Adm | Pub | Tre
  pure val PROVIDERS: Set[Acct] = Set(P1, P2)
  pure val CHALLENGERS: Set[Acct] = Set(Adm, Pub)
  pure val ACCTS: Set[Acct] = Set(P1, P2, Adm, Pub, Tre)

  // A plausible history of one bucket: cA, appended into cB, front-deleted
  // into cD. Roots are opaque identities.
  type Commitment = { root: int, start: int, count: int }
  pure val NO_COMMIT: Commitment = { root: 0, start: 0, count: 0 }
  pure val cA: Commitment = { root: 1, start: 0, count: 2 }
  pure val cB: Commitment = { root: 2, start: 0, count: 3 }
  pure val cD: Commitment = { root: 3, start: 2, count: 2 }
  pure val COMMITMENTS: Set[Commitment] = Set(cA, cB, cD)
  pure val ALL_SEQS: Set[int] = 0.to(3)

  pure def rangeEnd(c: Commitment): int = c.start + c.count
  pure def containsSeq(c: Commitment, s: int): bool = s >= c.start and s < rangeEnd(c)
  pure def seqsOf(c: Commitment): Set[int] =
    if (c.count == 0) Set() else c.start.to(rangeEnd(c) - 1)
  pure def imin(a: int, b: int): int = if (a < b) a else b

  // ── state ──────────────────────────────────────────────────────────────
  type ProviderSt = {
    registered: bool,          // present in Providers map
    stake: int,                // ProviderInfo.stake
    deregisterAt: int,         // -1 = not announced
    pending: int,              // PendingChallenges counter
    leaves: Set[int],          // ghost: seqs physically held off-chain
    signed: Set[Commitment],   // off-chain commitments this provider signed
    lastSync: Commitment,      // replica's last confirmed sync (NO_COMMIT = none)
  }

  type ChallengeSt = {
    provider: Acct,
    challenger: Acct,
    root: int,                 // challenged commitment (count is a ghost:
    start: int,                //  on-chain Challenge stores only root+start,
    count: int,                //  the MMR itself determines which leaves exist)
    leafIndex: int,
    deposit: int,
  }

  type Outcome =
    | DefendedProof | DefendedDeleted | DefendedSuperseded
    | TimeoutSlash | InvalidProofSlash | InvalidDeletionSlash | InvalidSupersededSlash
  type Response = RProof | RDeleted | RSuperseded

  // Ghost log entry, one per resolved challenge.
  type Resolution = {
    deadline: int,
    index: int,
    provider: Acct,
    challenger: Acct,
    outcome: Outcome,
    heldChallenged: bool,      // provider held the challenged seq at resolution
    heldAllUnderRoot: bool,    // provider held every leaf of the challenged commitment
    legitDeleted: bool,        // admin had signed a deletion past the challenged seq
    inRange: bool,             // leafIndex < challenged commitment's leaf_count
    stakeLoss: int,            // provider stake lost in this resolution
  }

  type Snapshot = { c: Commitment, signers: Set[Acct] }

  var now: int                          // anchor (relay) block
  var prov: Acct -> ProviderSt          // domain PROVIDERS
  var free: Acct -> int                 // free balances; Tre = treasury
  var reserved: Acct -> int             // reserved: stake + challenge deposits
  var canonical: Snapshot               // c == NO_COMMIT until first checkpoint
  var adminDeletedTo: int               // max new_start_seq the admin ever signed (0 = none)
  var open: (int, int) -> ChallengeSt   // Challenges double map: (deadline, index)
  var nextIdx: int -> int               // NextChallengeIndex per deadline (never decremented)
  var log: List[Resolution]             // ghost

  pure def isDefended(o: Outcome): bool =
    or { o == DefendedProof, o == DefendedDeleted, o == DefendedSuperseded }

  // ── functional layer ───────────────────────────────────────────────────

  // respond_to_challenge cost-split table (lib.rs).
  pure def challengerPct(t: int): int =
    if (t <= 1) 90 else if (t <= 5) 80 else if (t <= 24) 70 else if (t <= 95) 60 else 50

  // Proof adjudication. Code path: verify_merkle_proof binds chunk_index inside
  // the *submitted* leaf, but verify_mmr_proof never checks that the submitted
  // leaf sits at challenge.target.leaf_index — so any held leaf under the
  // challenged root verifies. Design path additionally requires the challenged
  // leaf itself (and hence that it exists in the commitment at all).
  pure def proofVerifies(held: Set[int], ch: ChallengeSt): bool = {
    val underRoot = seqsOf({ root: ch.root, start: ch.start, count: ch.count })
    val challengedSeq = ch.start + ch.leafIndex
    if (STRICT_PROOF)
      underRoot.contains(challengedSeq) and held.contains(challengedSeq)
    else
      held.intersect(underRoot).size() > 0
  }

  // Deleted adjudication (lib.rs): valid iff an admin-signed commitment with
  // new_start_seq > challenged_seq exists to present.
  pure def deletedVerifies(deletedTo: int, ch: ChallengeSt): bool =
    deletedTo > ch.start + ch.leafIndex

  // Superseded adjudication (lib.rs): a canonical snapshot exists, the
  // challenged root is not the canonical root, and the challenged seq is
  // inside the canonical range.
  pure def supersededVerifies(canon: Snapshot, ch: ChallengeSt): bool = and {
    canon.c.count > 0,
    ch.root != canon.c.root,
    containsSeq(canon.c, ch.start + ch.leafIndex),
  }

  type Bundle = {
    pv: Acct -> ProviderSt,
    fr: Acct -> int,
    rs: Acct -> int,
    lg: List[Resolution],
  }

  pure def mkRes(
    k: (int, int), ch: ChallengeSt, out: Outcome,
    pSt: ProviderSt, deletedTo: int, loss: int
  ): Resolution = {
    val underRoot = seqsOf({ root: ch.root, start: ch.start, count: ch.count })
    val challengedSeq = ch.start + ch.leafIndex
    {
      deadline: k._1,
      index: k._2,
      provider: ch.provider,
      challenger: ch.challenger,
      outcome: out,
      heldChallenged: pSt.leaves.contains(challengedSeq) and underRoot.contains(challengedSeq),
      heldAllUnderRoot: underRoot.subseteq(pSt.leaves),
      legitDeleted: deletedTo > challengedSeq,
      inRange: ch.leafIndex < ch.count,
      stakeLoss: loss,
    }
  }

  // slash_provider_for_failed_challenge + decrement_pending: full stake to the
  // Treasury, deposit refunded — but ONLY under `if let Some(provider_info)`;
  // for an unregistered provider the deposit stays reserved (modeled faithfully
  // so the leak would show if that state were reachable).
  pure def settleSlash(
    b: Bundle, k: (int, int), ch: ChallengeSt, out: Outcome, deletedTo: int
  ): Bundle = {
    val p = b.pv.get(ch.provider)
    val pDec = { ...p, pending: p.pending - 1 }
    if (p.registered) {
      val slashAmt = imin(p.stake, b.rs.get(ch.provider))
      {
        pv: b.pv.put(ch.provider, { ...pDec, stake: 0 }),
        fr: b.fr
          .put(Tre, b.fr.get(Tre) + slashAmt)
          .put(ch.challenger, b.fr.get(ch.challenger) + ch.deposit),
        rs: b.rs
          .put(ch.provider, b.rs.get(ch.provider) - slashAmt)
          .put(ch.challenger, b.rs.get(ch.challenger) - ch.deposit),
        lg: b.lg.append(mkRes(k, ch, out, p, deletedTo, slashAmt)),
      }
    } else {
      { ...b, pv: b.pv.put(ch.provider, pDec),
        lg: b.lg.append(mkRes(k, ch, out, p, deletedTo, 0)) }
    }
  }

  // respond_to_challenge defended path: challenger's deposit is split by
  // response time (challenger share → provider's free balance, remainder
  // refunded), and the provider-side share is SLASHED FROM STAKE into the
  // Treasury (lib.rs slash_reserved on `who`).
  pure def settleDefense(
    b: Bundle, k: (int, int), ch: ChallengeSt, out: Outcome, tNow: int, deletedTo: int
  ): Bundle = {
    val p = b.pv.get(ch.provider)
    val pDec = { ...p, pending: p.pending - 1 }
    val rTime = tNow - (k._1 - CHALLENGE_TIMEOUT)
    val chCost = ch.deposit * challengerPct(rTime) / 100
    val pCost = imin(ch.deposit * (100 - challengerPct(rTime)) / 100, b.rs.get(ch.provider))
    {
      pv: b.pv.put(ch.provider,
        { ...pDec, stake: if (p.stake > pCost) p.stake - pCost else 0 }),
      fr: b.fr
        .put(ch.provider, b.fr.get(ch.provider) + chCost)
        .put(ch.challenger, b.fr.get(ch.challenger) + (ch.deposit - chCost))
        .put(Tre, b.fr.get(Tre) + pCost),
      rs: b.rs
        .put(ch.challenger, b.rs.get(ch.challenger) - ch.deposit)
        .put(ch.provider, b.rs.get(ch.provider) - pCost),
      lg: b.lg.append(mkRes(k, ch, out, p, deletedTo, pCost)),
    }
  }

  pure def mapWithout(m: (int, int) -> ChallengeSt, k: (int, int)): (int, int) -> ChallengeSt =
    m.keys().exclude(Set(k)).mapBy(kk => m.get(kk))

  // ── actions ────────────────────────────────────────────────────────────

  action init: bool = all {
    now' = 1,
    prov' = PROVIDERS.mapBy(p => {
      registered: true,
      stake: STAKE,
      deregisterAt: -1,
      pending: 0,
      leaves: if (p == P1) ALL_SEQS else Set(),  // client uploaded to the primary
      signed: Set(),
      lastSync: NO_COMMIT,
    }),
    free' = Map(P1 -> 20, P2 -> 20, Adm -> 100, Pub -> 100, Tre -> 0),
    reserved' = Map(P1 -> STAKE, P2 -> STAKE, Adm -> 0, Pub -> 0, Tre -> 0),
    canonical' = { c: NO_COMMIT, signers: Set() },
    adminDeletedTo' = 0,
    open' = Map(),
    nextIdx' = Map(),
    log' = List(),
  }

  // Anchor clock advance + the on_initialize sweep: every challenge whose
  // deadline is now strictly in the past is slashed for Timeout.
  action advanceBlockBy(jump: int): bool = {
    val newNow = now + jump
    val expired = open.keys().filter(k => k._1 < newNow)
    val b = expired.fold(
      { pv: prov, fr: free, rs: reserved, lg: log },
      (acc, k) => settleSlash(acc, k, open.get(k), TimeoutSlash, adminDeletedTo)
    )
    all {
      jump > 0,
      now' = newNow,
      prov' = b.pv,
      free' = b.fr,
      reserved' = b.rs,
      log' = b.lg,
      open' = open.keys().exclude(expired).mapBy(k => open.get(k)),
      canonical' = canonical,
      adminDeletedTo' = adminDeletedTo,
      nextIdx' = nextIdx,
    }
  }

  // Primary signs an off-chain commitment (honest signing: only over data it holds).
  action signOffchainC(c: Commitment): bool = {
    val p = prov.get(P1)
    all {
      now < AGREEMENT_END,
      p.registered,
      seqsOf(c).subseteq(p.leaves),
      prov' = prov.put(P1, { ...p, signed: p.signed.union(Set(c)) }),
      now' = now, free' = free, reserved' = reserved, canonical' = canonical,
      adminDeletedTo' = adminDeletedTo, open' = open, nextIdx' = nextIdx, log' = log,
    }
  }

  // Client submits a checkpoint carrying the primary's signature; canonical
  // range only ever moves forward.
  action doCheckpointC(c: Commitment): bool = all {
    prov.get(P1).signed.contains(c),
    rangeEnd(c) >= rangeEnd(canonical.c),
    canonical' = { c: c, signers: Set(P1) },
    now' = now, prov' = prov, free' = free, reserved' = reserved,
    adminDeletedTo' = adminDeletedTo, open' = open, nextIdx' = nextIdx, log' = log,
  }

  // Replica syncs a primary-signed commitment and confirms on-chain
  // (confirm_replica_sync): fetches the data, records last_sync.
  action replicaSyncC(c: Commitment): bool = {
    val p = prov.get(P2)
    all {
      now < AGREEMENT_END,
      prov.get(P1).signed.contains(c),
      prov' = prov.put(P2, { ...p, lastSync: c, leaves: p.leaves.union(seqsOf(c)) }),
      now' = now, free' = free, reserved' = reserved, canonical' = canonical,
      adminDeletedTo' = adminDeletedTo, open' = open, nextIdx' = nextIdx, log' = log,
    }
  }

  // The environment: a provider silently loses one stored leaf.
  action loseLeaf(p: Acct, s: int): bool = {
    val ps = prov.get(p)
    all {
      ps.leaves.contains(s),
      prov' = prov.put(p, { ...ps, leaves: ps.leaves.exclude(Set(s)) }),
      now' = now, free' = free, reserved' = reserved, canonical' = canonical,
      adminDeletedTo' = adminDeletedTo, open' = open, nextIdx' = nextIdx, log' = log,
    }
  }

  // Admin signs a deletion commitment raising start_seq to s. Once signed, any
  // provider can present it as a Deleted defense for seqs below s.
  action adminDeleteTo(s: int): bool = all {
    s > adminDeletedTo,
    adminDeletedTo' = s,
    now' = now, prov' = prov, free' = free, reserved' = reserved,
    canonical' = canonical, open' = open, nextIdx' = nextIdx, log' = log,
  }

  // create_challenge: reserve deposit, allocate (deadline, index) under the
  // per-deadline cap, bump pending counters.
  action submitChallenge(challenger: Acct, provider: Acct, c: Commitment, leafIndex: int): bool = {
    val deadline = now + CHALLENGE_TIMEOUT
    val idx = if (nextIdx.keys().contains(deadline)) nextIdx.get(deadline) else 0
    val p = prov.get(provider)
    all {
      free.get(challenger) >= DEPOSIT,
      idx < MAX_PER_DEADLINE,
      free' = free.put(challenger, free.get(challenger) - DEPOSIT),
      reserved' = reserved.put(challenger, reserved.get(challenger) + DEPOSIT),
      open' = open.put((deadline, idx), {
        provider: provider, challenger: challenger,
        root: c.root, start: c.start, count: c.count,
        leafIndex: leafIndex, deposit: DEPOSIT,
      }),
      nextIdx' = nextIdx.put(deadline, idx + 1),
      prov' = prov.put(provider, { ...p, pending: p.pending + 1 }),
      now' = now, canonical' = canonical, adminDeletedTo' = adminDeletedTo, log' = log,
    }
  }

  // The three entry points. leafIndex ranges over 0..4 deliberately: none of
  // the extrinsics bounds target.leaf_index against the commitment's
  // leaf_count, so out-of-range challenges are accepted on-chain.
  action challengeCheckpointAs(challenger: Acct, leafIndex: int): bool = all {
    canonical.c.count > 0,
    canonical.signers.contains(P1),   // snapshot bitfield
    now < AGREEMENT_END,              // active agreement required
    submitChallenge(challenger, P1, canonical.c, leafIndex),
  }

  action challengeOffchainAs(challenger: Acct, c: Commitment, leafIndex: int): bool = all {
    prov.get(P1).signed.contains(c),  // provider signature verifies
    now < AGREEMENT_END,
    submitChallenge(challenger, P1, c, leafIndex),
  }

  action challengeReplicaAs(challenger: Acct, leafIndex: int): bool = {
    val ls = prov.get(P2).lastSync
    all {
      ls.count > 0,                   // last_sync record exists
      now < AGREEMENT_END,
      submitChallenge(challenger, P2, ls, leafIndex),
    }
  }

  // respond_to_challenge: consume the challenge, adjudicate, settle.
  action respondAs(k: (int, int), rsp: Response): bool = {
    val ch = open.get(k)
    val p = prov.get(ch.provider)
    val outcome = match rsp {
      | RProof => if (proofVerifies(p.leaves, ch)) DefendedProof else InvalidProofSlash
      | RDeleted => if (deletedVerifies(adminDeletedTo, ch)) DefendedDeleted else InvalidDeletionSlash
      | RSuperseded => if (supersededVerifies(canonical, ch)) DefendedSuperseded else InvalidSupersededSlash
    }
    val b0 = { pv: prov, fr: free, rs: reserved, lg: log }
    val b =
      if (isDefended(outcome)) settleDefense(b0, k, ch, outcome, now, adminDeletedTo)
      else settleSlash(b0, k, ch, outcome, adminDeletedTo)
    all {
      open.keys().contains(k),
      now <= k._1,                    // ensure!(anchor_block <= deadline)
      prov' = b.pv,
      free' = b.fr,
      reserved' = b.rs,
      log' = b.lg,
      open' = mapWithout(open, k),
      now' = now, canonical' = canonical, adminDeletedTo' = adminDeletedTo, nextIdx' = nextIdx,
    }
  }

  // deregister_provider: only with no live agreements (committed_bytes == 0).
  action announceDeregP(p: Acct): bool = {
    val ps = prov.get(p)
    all {
      ps.registered,
      ps.deregisterAt == -1,
      now >= AGREEMENT_END,
      prov' = prov.put(p, { ...ps, deregisterAt: now + DEREG_PERIOD }),
      now' = now, free' = free, reserved' = reserved, canonical' = canonical,
      adminDeletedTo' = adminDeletedTo, open' = open, nextIdx' = nextIdx, log' = log,
    }
  }

  // complete_deregister: window elapsed and no pending challenges.
  action completeDeregP(p: Acct): bool = {
    val ps = prov.get(p)
    all {
      ps.registered,
      ps.deregisterAt != -1,
      now >= ps.deregisterAt,
      ps.pending == 0,
      prov' = prov.put(p, { ...ps, registered: false, stake: 0 }),
      free' = free.put(p, free.get(p) + ps.stake),
      reserved' = reserved.put(p, reserved.get(p) - ps.stake),
      now' = now, canonical' = canonical, adminDeletedTo' = adminDeletedTo,
      open' = open, nextIdx' = nextIdx, log' = log,
    }
  }

  // ── nondeterministic step ──────────────────────────────────────────────

  action step: bool = {
    nondet challenger = CHALLENGERS.oneOf()
    nondet provider = PROVIDERS.oneOf()
    nondet c = COMMITMENTS.oneOf()
    nondet leafIndex = 0.to(4).oneOf()
    nondet jump = 1.to(2).oneOf()
    nondet newStart = 1.to(4).oneOf()
    any {
      advanceBlockBy(jump),
      signOffchainC(c),
      doCheckpointC(c),
      replicaSyncC(c),
      { nondet s = prov.get(provider).leaves.oneOf()
        loseLeaf(provider, s) },
      adminDeleteTo(newStart),
      challengeCheckpointAs(challenger, leafIndex),
      challengeOffchainAs(challenger, c, leafIndex),
      challengeReplicaAs(challenger, leafIndex),
      { nondet k = open.keys().oneOf()
        nondet rsp = Set(RProof, RDeleted, RSuperseded).oneOf()
        respondAs(k, rsp) },
      announceDeregP(provider),
      completeDeregP(provider),
    }
  }

  // ── invariants (design guarantees) ─────────────────────────────────────

  pure val TOTAL_ISSUANCE: int = 240 + 2 * 100  // init free + init reserved (STAKE=100 instances)

  // No tokens created or destroyed: slash_reserved + resolve_creating keeps
  // issuance whole (Treasury-routed).
  val moneyConserved: bool =
    ACCTS.fold(0, (acc, a) => acc + free.get(a) + reserved.get(a)) == TOTAL_ISSUANCE

  // Every reserved token is attributable: provider stake plus the deposits of
  // this account's open challenges. A violation = a leaked/stuck deposit.
  def depositsBy(a: Acct): int =
    open.keys().fold(0, (acc, k) =>
      acc + (if (open.get(k).challenger == a) open.get(k).deposit else 0))

  val reservedAccounted: bool = and {
    PROVIDERS.forall(p => reserved.get(p) == prov.get(p).stake + depositsBy(p)),
    Set(Adm, Pub, Tre).forall(a => reserved.get(a) == depositsBy(a)),
  }

  // A challenge resolves exactly once: no id resolved twice, and no resolved
  // id still open (defended AND timed out is the classic double-spend here).
  val resolvesOnce: bool = {
    val ids = log.foldl(List(), (acc, r) => acc.append((r.deadline, r.index)))
    and {
      ids.length() == ids.foldl(Set(), (s, i) => s.union(Set(i))).size(),
      log.foldl(true, (ok, r) => ok and not(open.keys().contains((r.deadline, r.index)))),
    }
  }

  // Design, Challenge Game: "Stake is never touched on a valid response."
  val stakeIntactOnDefense: bool =
    log.foldl(true, (ok, r) => ok and (isDefended(r.outcome) implies r.stakeLoss == 0))

  // Design: a successful defense means the provider was actually entitled to
  // one — it held the challenged leaf, or the admin had signed its deletion.
  // (dishonestAlwaysSlashable, resolution-level safety core.)
  val defendedImpliesEntitled: bool =
    log.foldl(true, (ok, r) =>
      ok and (isDefended(r.outcome) implies (r.heldChallenged or r.legitDeleted)))

  // Design: an honest provider — holding every leaf of the commitment it is
  // challenged on and responding in-window with a Proof — is never slashed.
  val honestNeverSlashed: bool =
    log.foldl(true, (ok, r) =>
      ok and not(r.outcome == InvalidProofSlash and r.heldAllUnderRoot))

  // A provider can never exit (deregister) while a challenge against it is
  // open or its pending counter is non-zero.
  val noExitWithLiability: bool =
    PROVIDERS.forall(p => or {
      prov.get(p).registered,
      and {
        prov.get(p).pending == 0,
        open.keys().forall(k => open.get(k).provider != p),
      },
    })

  val allInvariants: bool = and {
    moneyConserved,
    reservedAccounted,
    resolvesOnce,
    stakeIntactOnDefense,
    defendedImpliesEntitled,
    honestNeverSlashed,
    noExitWithLiability,
  }

  // ── witnesses (reachability / coverage) ────────────────────────────────

  val witnessDefended: bool = log.foldl(false, (b, r) => b or isDefended(r.outcome))
  val witnessTimeoutSlash: bool = log.foldl(false, (b, r) => b or r.outcome == TimeoutSlash)
  val witnessInvalidSlash: bool = log.foldl(false, (b, r) =>
    b or r.outcome == InvalidProofSlash or r.outcome == InvalidDeletionSlash
      or r.outcome == InvalidSupersededSlash)
  val witnessDeletedDefense: bool = log.foldl(false, (b, r) => b or r.outcome == DefendedDeleted)
  val witnessSupersededDefense: bool = log.foldl(false, (b, r) => b or r.outcome == DefendedSuperseded)
  val witnessDeregistered: bool = PROVIDERS.exists(p => not(prov.get(p).registered))
  val witnessOutOfRangeChallenge: bool = log.foldl(false, (b, r) => b or not(r.inRange))
  // MaxChallengesPerDeadline reached: further challenges created at that
  // anchor block bounce with TooManyChallengesThisBlock (censorship window).
  val witnessCapFull: bool = nextIdx.keys().exists(d => nextIdx.get(d) >= MAX_PER_DEADLINE)
}

// Code semantics: verify_mmr_proof unbound to the challenged leaf_index.
module challengesCode {
  import challenges(
    CHALLENGE_TIMEOUT = 3,
    DEREG_PERIOD = 4,
    DEPOSIT = 10,
    STAKE = 100,
    MAX_PER_DEADLINE = 2,
    AGREEMENT_END = 8,
    STRICT_PROOF = false
  ).*
}

// Design semantics: proof bound to the challenged leaf (implementation.md
// "Verification" pseudocode passes challenge.leaf_index into verify_mmr_proof).
module challengesDesign {
  import challenges(
    CHALLENGE_TIMEOUT = 3,
    DEREG_PERIOD = 4,
    DEPOSIT = 10,
    STAKE = 100,
    MAX_PER_DEADLINE = 2,
    AGREEMENT_END = 8,
    STRICT_PROOF = true
  ).*
}
