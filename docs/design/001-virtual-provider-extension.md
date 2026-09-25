# Virtual Provider Extension for Scalable Web3 Storage

| Field | Value |
| --- | --- |
| **Authors** | eskimor |
| **Status** | Draft |
| **Related** | [Scalable Web3 Storage](./scalable-web3-storage.md), [Implementation Details](./scalable-web3-storage-implementation.md), [key theft #300](https://github.com/paritytech/web3-storage/issues/300) |

---

## Motivation

Two independent problems, one mechanism.

**Legal takedowns.** A failed challenge slashes a provider's entire stake ([Provider Stake](./scalable-web3-storage.md#provider-stake)). If a court orders a provider to take down data illegal in its jurisdiction, obeying means failing challenges — slashed for following the law. A provider should not have to choose between losing money and prison over a random user's upload.

**Key theft ([#300](https://github.com/paritytech/web3-storage/issues/300)).** Hot/cold keys leave one risk: an adversary with the stolen hot key commits to garbage data, getting the provider slashed for the unanswerable challenge (griefing). Impossible with a single stolen key once commitments need a threshold of signatures.

A virtual provider mutualizes both: independent physical providers back one logical provider, share its stake, and *any* of them can defend a challenge. If one member goes dark — court order, key loss, outage — the others still serve and the group is not slashed. An insurance pool for providers.

---

## Model

A **virtual provider** is a first-class provider identity backed by up to `MaxPhysicalMembers` **physical providers** (its *members*). No chain-enforced minimum: a single founder creates it and it simply does not accept agreements until enough co-members have joined. It has:

- a **synthetic account** (`AccountId` derived from a `PalletId` + a virtual-provider id, like the treasury account), so every existing `AccountId`-keyed extrinsic, agreement, challenge, and runtime-API query addresses it unchanged;
- a **`per_provider_stake`** slice each member puts at risk out of its own physical stake;
- an advertised **`stake`** (the normal provider stake field) — the backing behind any one commitment.

From the two, `k = ceil(stake / per_provider_stake)` — the signers needed so their slices cover the advertised stake; a commitment is valid only if signed by ≥`k` members, and a failed challenge slashes every signer by `per_provider_stake` — at least `k` of them, so at least `stake`. The pallet enforces that this derived `k` is a **strict majority** (`k > members.len() / 2`), so any two valid bundles share a signer ([Stake and Slashing](#stake-and-slashing)). So `stake` cannot be set so low (relative to `per_provider_stake` and member count) that `k` falls to half or below.

The synthetic account is the only thing clients and buckets reference; members are internal. A bucket lists it in `primary_providers`; in the checkpoint layout it expands to one slot per member ([Checkpoints](#checkpoints)).

**Liability lives in the agreement, not in membership.** As the base design snapshots stake into each agreement (price is prepaid and needs no snapshot), a virtual agreement additionally snapshots **the member set and `per_provider_stake` in force when it was struck** (and the base `agreement_id`). A commitment binds to its `agreement_id` ([base "Storage Agreements"](./scalable-web3-storage.md#storage-agreements)), hence to a fixed, known member set. So members join and leave freely: leaving the live `members` list only affects *future* agreements — a member stays liable through every agreement whose snapshot names it, until that agreement ends. This is the ordinary base contract ("no early exit; liable until your agreements expire") applied per member, and it removes any need to freeze membership or reason about who saw which off-chain commitment.

A virtual provider has **no address of its own** — clients reach it through members. The discriminant rides `ProviderInfo`'s endpoint field:

```rust
enum ProviderEndpoint<T: Config> {
    /// A single real provider: its own multiaddr. Today's behaviour.
    Physical(BoundedVec<u8, T::MaxMultiaddrLength>),
    /// A virtual provider: no address of its own. The `VirtualProvider` payload
    /// lives in the `VirtualProviders` side map, keyed by the same account;
    /// member multiaddrs are read from each member's own `Providers[m]` entry.
    Virtual,
}
// ProviderInfo.multiaddr: BoundedVec<...>  becomes  ProviderInfo.endpoint: ProviderEndpoint<T>

struct VirtualProvider<T: Config> {
    /// Slice each member puts at risk out of its own physical stake. Join
    /// requires `Providers[m].stake >= per_provider_stake`. No additional hold
    /// is placed — the member's existing `ProviderStake` hold backs this
    /// membership alongside its direct agreements and any other virtual
    /// providers it belongs to.
    per_provider_stake: BalanceOf<T>,
    /// After a lowering of `per_provider_stake`: the old value and the block
    /// until which live agreements may still carry it. Members stay bound to
    /// it until then ([Stake and Slashing](#stake-and-slashing)).
    higher_pps_lock: Option<(BalanceOf<T>, BlockNumberFor<T>)>,
    /// Advertised backing lives in the base `ProviderInfo.stake`. Together with
    /// `per_provider_stake` it yields `k = ceil(stake / per_provider_stake)`
    /// (not stored). Invariant: `k` a strict majority of `members.len()`.
    /// Current backing set — the members of *new* agreements, all registered
    /// Physical providers. Live agreements keep the set they snapshotted.
    members: BoundedVec<T::AccountId, T::MaxPhysicalMembers>,
    /// Off-chain coordination endpoint (chat/group) for members to agree on
    /// settings, replacements, desired stake and pricing, ...
    coordination_channel: BoundedVec<u8, T::MaxCoordChannelLen>,
}

/// Entry of `ProviderVirtuals[member]`: one per virtual provider the member is
/// in or has left. Exists exactly while the member can be slashed for it.
struct VirtualMembership<T: Config> {
    virtual_provider: T::AccountId,
    state: MembershipState<T>,
    /// Sum of the non-reimbursed challenge response fees this member has paid
    /// for this virtual provider. Advisory: drives the off-chain response order
    /// and kick decisions ([Who answers](#who-answers)).
    response_cost_borne: BalanceOf<T>,
}

enum MembershipState<T: Config> {
    /// In the live set; liable at the virtual's current `per_provider_stake`,
    /// or `higher_pps_lock` while that is unexpired and higher.
    Active,
    /// Left the live set (leave, kick or auto-removal); liable through the
    /// agreements that snapshot it, at `stake_at_risk` each, until `until`.
    Leaving { until: BlockNumberFor<T>, stake_at_risk: BalanceOf<T> },
}
```

The `VirtualProvider` payload is a side map, not an inline `ProviderInfo` enum variant, so it does not enter every provider's `MaxEncodedLen`. `ProviderInfo` stays in the `Providers` map — `Physical` is the untouched default, the change additive.

### Members are registered physical providers

Each member is an ordinary `Physical` provider. Two rules:

- **`Physical` only** — no virtual-in-virtual; joins reject a `Virtual` candidate (recursion makes slashing/stake lookup unbounded and hides the real backing).
- **Both roles, up to `MaxVirtualsPerProvider` virtuals** — a provider can serve direct agreements *and* belong to virtual providers on the same stake. A virtual failure slashes only `per_provider_stake`, so a member's other virtuals usually remain intact, whereas a *direct-agreement* failure slashes its whole stake and drops it from all of them. A provider is therefore self-incentivized to keep its direct-agreement risk low once it joins a virtual provider.

  The cap is 2, which covers the concrete use case — moving from one virtual provider to another without downtime, by belonging to both during the transition. One membership already provides the full insurance-pool property (any member defends; a slash costs `per_provider_stake`, not everything), so more than 2 buys only diversification across pools, which no current use case requires. The cap also keeps the slash cascade O(1): slashing a member walks every virtual it belongs to, so an unbounded count would require a budgeted sweep with carry-over, like the base slash sweep. The bound therefore exists for weight, not economics, and is raiseable — the membership list is the `ProviderVirtuals` side map rather than an inline `ProviderInfo` field, so raising the cap does not grow every provider's `MaxEncodedLen`, and the sweep can be added when it is.

A member need not accept any direct business: it sets `accepting_primary = false` and `replica_sync_price = None` on its own `ProviderInfo` and discovery skips it. The virtual provider's own `ProviderSettings` — price, `accepting_primary`, replica price — are set through `set_virtual_settings` and are independent of the members'.

---

## Stake and Slashing

**A failed challenge slashes every signer of the challenged commitment by `per_provider_stake`.** Uniform — same exposure, same loss, regardless of a member's total stake. A valid bundle has at least `k` signers, so at least `k * per_provider_stake` = the advertised `stake` is slashed: `stake` is the floor on the backing behind any commitment, and since all members sign what they store ([Commitments](#commitments-threshold-signatures)) the usual backing is `n * per_provider_stake`. The signers are known: the commitment names its `agreement_id`, whose snapshot fixes the eligible set, and the bundle — or, for a checkpoint, its set bits in the slot layout — names which of them signed. A challenger could present only `k` of the signatures it holds and let the rest walk; so, mirroring `extend_checkpoint`, **`extend_challenge`** lets anyone add further signatures over the same payload from the snapshotted set while the challenge is open. A signer facing the slash pulls in its co-signers, and every signer ends up slashed.

**Liability is enforced on members, not on the virtual account.** The invariant: for every live agreement, each member it names holds at least the `per_provider_stake` that agreement snapshotted. Three pieces enforce it in O(1), without walking agreements:

- **The virtual tracks two dates.** `cur_until`: the max expiry over its live agreements — the base field, never reset here because the pallet writes the virtual's `stake` directly (`set_virtual_settings`, leave/kick/auto-removal, the `n = 1` rewrite) without the `set_stake` bookkeeping. `higher_pps_lock`: after a lowering of `per_provider_stake`, the old value and `cur_until` at that time — the highest value a live agreement may still carry, and until when.
- **Each member's `ProviderVirtuals` entry says what it owes that virtual** ([Membership Governance](#membership-governance)). `Active`: the virtual's live `per_provider_stake`, or `higher_pps_lock` if unexpired and higher. `Leaving { until, stake_at_risk }`: a fixed amount until a fixed block, both copied from the virtual at the moment it left.
- **Two base extrinsics read the entries.** `set_stake` may not lower a member's stake below what any entry says it owes; `deregister_provider` is rejected while any entry is `Active` or an unexpired `Leaving`.

The virtual's own `stake` holds nothing and needs no lock. Changing `per_provider_stake` through `set_virtual_settings`: raising requires every live member's stake to cover the new value; lowering writes `higher_pps_lock = (old, cur_until)` and is rejected while a previous lock is unexpired — the base stake-lowering rule, applied to the figure members are actually bound to.

**`k` is a strict majority.** Any two valid bundles then share a signer, so the group cannot split into two disjoint sets producing conflicting commitments, checkpoints or governance decisions — each backed by the full advertised `stake`, which only one of them can cover.

**Slashing cascades to a member's other backings.** A member's stake is shared across everything it backs. Losing `per_provider_stake` here lowers what remains; if the remainder falls below another virtual's `per_provider_stake`, the member can no longer cover that slice and is **auto-removed from that virtual's live `members`** (affecting only new agreements — existing ones keep their snapshot and the member's residual liability). A member slashed for its *own* direct agreement loses its whole stake and is auto-removed from all its virtuals. The cascade *is* the team-vetting incentive: a member bears the risk of whom it pools with.

Auto-removal takes `n` members to `n' = n - 1` and sets `k' = min(k, n')`, `stake' = k' * per_provider_stake`. So `stake` is unchanged unless the group required unanimity (`k = n`), where it drops by exactly one slice: `stake` is quantized in units of `per_provider_stake`, and `k` cannot fall while remaining a strict majority of `n'` unless there was no slack. Because `k'` is reachable by construction, the group never lands below its own threshold, and every `k`-approved call (join, kick, `set_virtual_settings`, dissolve) stays available. The removed member's `ProviderVirtuals` entry turns `Leaving`; it stays bound to the agreements that name it.

**The last member is not removed.** At `n = 1` auto-removal would leave an empty group with no one able to act, so the member stays and the figures are rewritten to what it can still cover: `per_provider_stake' = min(per_provider_stake, Providers[m].stake)` and `stake' = per_provider_stake'`. This is a forced lowering of `per_provider_stake`, so it folds into `higher_pps_lock` with `max` rather than waiting for the previous lock: the member stays bound to the old value for the agreements that carry it. Live agreements keep their snapshotted figures, so the rewrite applies to new agreements only. The result may be a zero-stake virtual provider.

**Clients learn about a slash from provider state, not (only) the event log.** Both a slash of the provider itself and (for a virtual) a slash of one of its members elsewhere bump `ProviderInfo.last_stake_event`. A client returning after arbitrary downtime walks its own agreements, reads each provider's field, and compares against the last block it checked, bounded by its agreement count. The field is deliberately coarse: it reports that backing *may* have been reduced, not that any particular agreement lost any. To determine actual exposure the client reads the agreement's snapshotted signer set and those members' current stake, which [Discovery](#discovery) surfaces per agreement. A member slashed elsewhere leaves live agreements naming it under-backed relative to their snapshot even when the virtual's own advertised `stake` is unchanged — which is why the bump is not conditional on `stake` moving.

A member complying with a takedown is protected only if another member serves the chunk; if none does, the signers of the challenged commitment are slashed (it may be one of them) — the extension cannot let content vanish for free. A complying member remains visible off-chain: its `response_cost_borne` stops growing while the others' does. Whether the group tolerates that is a member decision.

### Capacity

Not partitioned, and not aggregated across roles. A virtual agreement's bytes count into the **virtual provider's** `committed_bytes`; a member's own counter is unaffected by what the virtuals it belongs to commit to. Nothing is enforced against either figure — the base enforces no stake-per-byte requirement at all ([base "Stake vs. capacity"](./scalable-web3-storage-implementation.md)) — so aggregating them would spend weight on every member's counter at each virtual agreement create, expire and auto-removal for no gain. A client wanting a physical provider's total adds its direct `committed_bytes` and those of the virtuals it belongs to, read off `ProviderVirtuals`.

---

## Commitments: Threshold Signatures

Today a commitment is one provider signature over a `CommitmentPayload`, verified against that provider's `public_key`. A virtual commitment is a **bundle of ≥`k` member signatures** over the same payload, each verified against the respective member's key (`k` from [Model](#model)). Only a bundle meeting `k` is a valid commitment for the virtual provider.

`k` is the validity minimum, not a target. **All members sign every commitment they store** — signing is the service, and a member that does not is the freeloader [Payment](#payment) lets the others kick. The bundle carries every signature collected and every signer is liable, so `stake = k * per_provider_stake` is the floor on backing and `n * per_provider_stake` the norm; `k` is what the group still guarantees with `n - k` members down.

This is the key-theft mitigation: one stolen member key produces one signature, below `k`, so it cannot mint a fraudulent commitment.

### Write path

The base "immediate guarantee from one signature" ([design doc](./scalable-web3-storage.md#the-chain-as-credible-threat)) becomes a short collection round:

1. Client uploads to one member — the **coordinator** for this write. (A virtual provider is a replication set, so the data reaches *all* members regardless of which one is picked.)
2. The coordinator fans the data out and collects signatures over the new `CommitmentPayload` (which names the virtual provider's `agreement_id`).
3. Once it holds ≥`k` signatures it returns the **bundle** — the client's guarantee — with every signature collected so far, and hands the same bundle to every member, so each signer holds proof of its co-signers ([Stake and Slashing](#stake-and-slashing)). Waiting briefly past `k` for stragglers trades latency for backing; a group setting, not a protocol rule. A member that keeps missing the bundle bears less risk for the same pay; coordinators (chosen per write, so rotating) see this, and signer sets are on-chain in every checkpoint and challenge response.

**If collection stalls** (member down, or coordinator withholds): a sub-`k` bundle is not a commitment, so the client simply retries via a *different* coordinator (data is idempotent). A coordinator can withhold but not forge — every signature is checked against member keys by client and chain.

### Checkpoints

The checkpoint path does not distinguish virtual from physical. `checkpoint` / `extend_checkpoint` keep the base signature format, a list of `(AccountId, Signature)` pairs; the client collects them from its physical primaries directly and from a coordinator for a virtual one. The bucket's **slot layout** is derived, not stored: `primary_providers` expanded in order, a physical primary as one slot, a virtual primary as its agreement's snapshotted members in snapshot order. Bit `i` of `primary_signers` means slot `i` signed. Verification maps each pair to its slot and checks the signature against that account's key; slashing after a failed checkpoint challenge walks the challenged provider's set bits, each signer at its slot's amount — whole stake for a physical, snapshotted `per_provider_stake` for a member. No bundle type, no per-slot bitmask.

One rule is virtual-specific: **per virtual primary, a submission is all-or-nothing** — in `checkpoint` and `extend_checkpoint` alike, a virtual's signatures in one call are either ≥`k` or absent; a call carrying fewer than `k` for any group is rejected. So a bit is set only when its group reached `k` in that call, and set means liable, nothing more to interpret. `extend_checkpoint` can add further members of a group already in, or bring a whole group in at ≥`k`; it never accumulates toward `k`. The rule is what keeps a stolen member key harmless here: without it an attacker acting as a client — own bucket, own agreement with the virtual — could checkpoint garbage under that one signature and get the member slashed, the griefing that threshold signatures exist to prevent.

`min_providers` is the popcount — **physical signers**, members individually. A virtual no longer counts as one: redundancy is read uniformly from the bitfield — independent physical signers — for physical and virtual primaries alike. A client with one 4-member virtual sets `min_providers` in physical terms; below `k` it is moot, above `k` a real extra requirement.

`MaxPrimarySlots` (8) bounds the expanded layout, the bitfield and the signatures verified per checkpoint, replacing the base `MaxPrimaryProviders`. With `MaxPhysicalMembers = 4`, two full virtuals fit side by side — the migration case — as do a virtual plus four physical primaries, or eight physical. The layout only grows on an in-place extension (members joined) or a replacement activation (larger set); both are rejected if the result would exceed `MaxPrimarySlots`, so a bucket owner is never forced to accept it. A re-snapshot of a virtual's member set changes its slots; as for a primary removal in the base, the current snapshot's bits are adjusted in place on that extrinsic.

---

## Challenges

A challenge targets the **virtual account** (existing `challenge_checkpoint` / `challenge_offchain` / `challenge_replica`, unchanged). It resolves against the **member set snapshotted in the challenged `agreement_id`**: who is liable is fixed when the agreement is struck and unaffected by later membership churn, so no membership freeze is needed. On failure the `k` signers are each slashed `per_provider_stake`. **Any snapshotted member may respond**; the group is slashed only if none does. Members that joined later are not eligible — they are not in the snapshot, cannot sign for the agreement, and hold none of its data.

### Who answers

The chain assigns no duty. It records one figure per member and virtual provider, `response_cost_borne` in the member's `ProviderVirtuals` entry: the sum of the non-reimbursed shares of the response fees it has paid for this virtual provider (known at resolution, since the pallet computes the reimbursement). Members use it to decide, off-chain, who goes first.

**The convention (provider-node default).** On `ChallengeCreated`, every snapshotted member computes the same **candidate order** from state at the creation block: the snapshotted set, `Active` members first, then those in `Leaving`, each group sorted by `response_cost_borne` ascending (no entry = 0), ties by position in the snapshot. Leavers remain liable and eligible, so the order is never empty; they go last because no kick lever remains on them. Candidate `i` submits its response `i · L` anchor blocks after creation (`L = 2`), unless the challenge is already resolved or a response for it is already in its transaction pool. The member that has paid least goes first; whoever pays moves back for the next challenge. Fair in expectation, with no coordination round and no clock in the protocol itself.

**Why members follow it.** Responding costs the responder `c(t)`, the provider-borne share of the response fee, rising from 10% to 50% with latency (base cost table). Not responding costs nothing while another member does. Nobody responding costs every signer `per_provider_stake`, orders of magnitude more. So a member behind the others cannot gain by waiting: either it still ends up answering, at higher `c(t)`, or another member answers and its lag — visible on-chain to the peers who can kick it — grows. A member ahead of the others gains nothing by answering early; it only pays. Given a credible kick threat, which the kick mechanism assumes anyway, the order is self-enforcing, and the slash asymmetry guarantees that some member answers before the deadline whatever the others do. Deviation is harmless: a member that races anyway pays more and drops back; one that never answers falls behind and is kicked.

**Nobody can be targeted.** The order is derived from the cost figures, which a challenger can only move by paying for a real response. It can read who is candidate 0 and challenge then; that member's figure rises and it stops being candidate 0. Each member therefore answers about 1/`n` of the costly challenges. Inflating one's own figure by challenging one's own virtual provider costs ~100% of a response fee to gain ~10%.

**Public challenges** are reimbursed in full, so they add 0 to the figure and need no special case. The order still applies, to avoid duplicate work.

**Duplicate responses cost nothing.** The base response transaction extension ([impl doc](./scalable-web3-storage-implementation.md#response-transaction-extension)) gives every response to the same challenge the same `provides` tag, so a second one is rejected at pool import like a duplicate nonce, pruned from every pool once the first is included, and never charged. `L` therefore need not guarantee ordering — it only keeps honest members from wasting bandwidth — and a collision is not an error.

### Cost split

Unchanged from base ([Challenge Game](./scalable-web3-storage.md#the-challenge-game)): the virtual account is the `provider`; tiering and the ≥50% floor apply. The responding member pays its response fee and is reimbursed from the challenger's deposit exactly as a physical provider is. Whatever it bears stays with it and is added to its `response_cost_borne`; the response order, not redistribution, shares the cost. The agreement's payment is no source for it — it settles at expiry and may be burned entirely.

### Residual key-theft surface

Threshold commitments close the forged-commitment path. What one stolen member key can still do, all short of a slash:

- **not respond** → another snapshotted member answers;
- **forge a replica sync** → hits only that member's *own* replica agreements, not the `k`-signed virtual commitments;
- **trigger governance** → needs `k`-of-members ([Membership Governance](#membership-governance)).

So a stolen key can degrade service but cannot slash — the concrete gain over a lone provider.

---

## Payment

Each agreement's payment accrues to the synthetic account and is split **equally among the members it snapshotted** — natural, since they store the same data and risk the same `per_provider_stake`. Challenge-response fees are not redistributed here — they stay with the member that paid them, and the response order evens them out over time ([Who answers](#who-answers)).

Payment follows the snapshot, period: a member named in an agreement is paid for it whether it later left, was kicked, or was auto-removed — it carried the liability. Kicking a **freeloader** (never signs / never responds) therefore changes nothing already struck; it only keeps the member out of future agreements. Since a kick cannot take money, a majority gains nothing by kicking an honest member. The remaining signal is reputational — a kicked-count on the provider's own record, so other groups can see it before admitting it; its exact shape is open.

---

## Membership Governance

Changes to the **live `members`** set affect only *future* agreements — existing agreements keep their snapshot ([Model](#model)). So membership churns freely; the only invariant is that the live set can still sign: **`members.len() >= k`** with `k = ceil(stake / per_provider_stake)` a strict majority. All changes except `leave_virtual` are **`k`-of-members authorized** (never a single key). `stake` and `per_provider_stake` (hence `k`) may be adjusted in the same call as a membership change, so the set is never momentarily under-`k` or below majority.

**A member leaving bumps the provider's `version`** (base [Term Pinning](./scalable-web3-storage-implementation.md)). Composition is part of what a client buys: `3`-of-`4` is more resilient than `3`-of-`3` even at identical `k` and `stake` (one more member can go dark before the group can't cover). So a departure — whether or not it also lowers `stake` — is a worse-terms change a client may have declined, and its pinned request/extension correctly fails. A join (more redundancy, strictly better) does not bump. This is in fact the sharpest reason `version` exists: nothing else captures a composition change.

Two liabilities to separate:

- **Signing new agreements** — needs a live set of `>= k`.
- **A leaver's residual liability** — a member that left the live set is still liable through every agreement whose snapshot names it, until that agreement ends. Its `ProviderVirtuals` entry turns `Leaving { until, stake_at_risk }` instead of being removed. `until` is the virtual's `cur_until` at that moment: an upper bound on the expiry of every agreement naming it, since all were struck while it was live and an agreement naming a member that has left cannot be extended in place ([Changing a live agreement's member set](#changing-a-live-agreements-member-set)). `stake_at_risk` is the highest `per_provider_stake` those agreements may carry (the live value, or `higher_pps_lock` if unexpired). The entry blocks `set_stake` below `stake_at_risk` and `deregister_provider` until `until`, then is pruned by the next `join_virtual`, `set_stake` or `deregister_provider` of that member. O(1) to create, O(1) to check. The bound is conservative — it also covers agreements struck before the leaver joined — which is the price of not walking agreements; the figure is public before joining. A `Leaving` entry keeps its `MaxVirtualsPerProvider` slot until it is pruned.

### Create / join / kick

- **Create:** a founder (`Physical`) calls `create_virtual` with `stake` and `per_provider_stake`; the pallet derives the synthetic account, writes the `VirtualProvider` (founder as sole member), registers a `Virtual` `ProviderInfo`, not accepting. It starts accepting once `members.len() >= k`.
- **Join** (`join_virtual`): candidate must be `Physical` with `stake >= per_provider_stake`. While bootstrapping (not yet accepting, `members.len() < k`) the founder approves joins; once operational, joins are `k`-approved like other changes.
- **Kick** (`k`-approved): moves a member to `Leaving`, lowering `stake` by one slice if its slice was needed (`k' = min(k, n')`, as for auto-removal). No challenge freeze — a kicked member keeps its residual liability, so nothing is shed.

### Leaving

A member can always leave, alone, with `leave_virtual` — it is the one membership change that needs no `k`-approval, so nobody is held hostage. It moves to `Leaving` and, if its slice was needed to reach `stake`, `stake` drops by one slice (`k' = min(k, n')`, the auto-removal rule); that is always allowed because the virtual's `stake` has no lock ([Stake and Slashing](#stake-and-slashing)). It drops out of signing immediately but stays bound to what it already backs until expiry — never worse off than a lone provider. Because clients contracted with the *virtual* account, this churn never breaks the client-facing "data stays until expiry".

### Changing a live agreement's member set

`extend_agreement` re-snapshots current terms — for a virtual, the current `members`. In place, that is safe only if no signer is dropped: **an in-place extension is accepted only if every snapshotted member is still in the live set** (members may have joined, none left). Otherwise the old signers would leave the eligible set, every commitment they signed would stop verifying, and the client would hold no guarantee until the new set had signed. Such an extension is rejected; the client uses a **replacement**.

A replacement is the base mechanism ([impl doc](./scalable-web3-storage-implementation.md#replacement-agreements)), here with a new member set:

1. The owner creates it: a pending successor stored in the agreement record — fresh `agreement_id`, the current member set and `per_provider_stake`, a duration, prepaid. Not live: its members carry no liability, commitments naming it are not yet valid, it occupies no slots. The old agreement runs on, fully challengeable.
2. The new members sync the data and sign. The first `checkpoint` carrying ≥`k` of their signatures **activates** the replacement: in that block the record becomes the new agreement — new id, new snapshot, `expires_at = now + duration` — and the new set is liable for the tip. Continuous guarantee, no gap.
3. The old agreement ends there with extension semantics: its elapsed period is paid to its snapshotted members, equal split, leavers included; the unelapsed remainder rolls into the successor's escrow and is paid to the new set over the new term. Nothing is refunded, nothing charged twice.
4. If the old agreement expires first, the replacement activates then — it is the continuation the client paid for, and the new set is bound from that block whether or not it has signed, as with any agreement that has no commitment yet.

So a client starts a swap early enough for the new set to sync before the old agreement expires. Only `checkpoint` ever verifies signatures against a pending set, and only to activate it.

### Dissolution

`k`-approved once the synthetic account has no live agreement (`committed_bytes == 0`) and no open challenge: removes the `VirtualProviders` entry, the synthetic `ProviderInfo`, and the `Active` entries in its members' `ProviderVirtuals` (`n ≤ 4` writes). `Leaving` entries are all expired by then and prune themselves. No funds move — nothing was escrowed.

---

## Role of Multiple Primaries

Multiple primaries lose importance: a client chasing stake for an important bucket usually lands on one virtual primary that already carries decentralization internally, rather than hand-assembling physical ones. They stay useful where the client wants direct control — zero-downtime migration (run two virtual providers during the switch), or existing trust relationships (a provider it knows or runs).

---

## Discovery

Clients select on **stake**, unchanged — virtual-ness is not a selection axis (a low-stake virtual provider is no better than a physical one of equal stake). Because high stake gives a provider strong reason to pick independent backers, the highest-stake providers will tend to be virtual, so the decentralization dividend comes for free from chasing stake. Discovery therefore just makes a virtual provider's stake legible and exposes its internals. Additive changes:

- `ProviderInfoResponse` gains a physical/virtual discriminant. A virtual provider already reports its `stake` in the existing field (so stake sorting/matching works unchanged); it adds `per_provider_stake`, `k` and `member_count`.
- Member accounts (and voluntary jurisdiction attestations) may optionally be exposed as transparency — granularity is a per-deployment choice, defaulting to count + aggregate (independence is provider-self-attested in the base design anyway).
- `find_matching_providers` needs no virtual-specific scoring — a virtual provider competes on aggregate stake like any other.

---

## On-Chain Changes

Concrete additions the base pallet needs. All additive — `Physical` behaviour is unchanged.

| Area | Change |
|---|---|
| `ProviderInfo` | `multiaddr` becomes `endpoint: ProviderEndpoint<T>` (`Physical(multiaddr)` \| `Virtual`) — the tag is the discriminant. A virtual provider's base `stake` field holds `k * per_provider_stake`, kept in sync on any `k`/`per_provider_stake` change. New `last_stake_event: Option<BlockNumberFor<T>>` (see [Stake and Slashing](#stake-and-slashing)). |
| `VirtualProviders` map | New `StorageMap<AccountId, VirtualProvider<T>>` (`per_provider_stake`, `higher_pps_lock`, member accounts, coordination channel), keyed by the synthetic account; loaded only when members are needed. Invariant: `k` a strict majority of `members.len()`. The pallet writes the virtual's `stake` field directly, without the base `set_stake` bookkeeping, so its `cur_until` is never reset and is the max expiry over its live agreements. |
| `StorageAgreement` (virtual) | Additionally snapshots the **member set** and `per_provider_stake` in force at creation/extension (base already snapshots `agreement_id`, price, stake). Liability and the set of eligible responders resolve against this snapshot, not the live set. |
| Synthetic account | `PalletId` + virtual-provider id (treasury-style), so all `AccountId`-keyed extrinsics/queries work unchanged. |
| New extrinsics | `create_virtual` (founder), `join_virtual`, `leave_virtual` (the leaver alone), `kick_member`, `set_virtual_settings`, `dissolve_virtual` — all but `leave_virtual` `k`-of-members authorized. `set_virtual_settings` takes `stake` and `per_provider_stake` together; lowering `per_provider_stake` sets `higher_pps_lock = (old, cur_until)` and is rejected while a previous lock is unexpired, raising requires every live member's stake to cover the new value. Enforce `Providers[m].stake >= per_provider_stake` per member, `members.len() >= k`, `MaxVirtualsPerProvider` on the candidate (expired `Leaving` entries pruned first), and reject a `Virtual` candidate. |
| Commitment verification | For a virtual account, verify a **`k`-signature bundle** (signers ∈ the `agreement_id`'s snapshotted set) over `CommitmentPayload` instead of a single signature. |
| `checkpoint` / `extend_checkpoint` | Signature format unchanged (`(AccountId, Signature)` pairs). Slots derived from `primary_providers`, a virtual expanded to its snapshotted members; a virtual's signatures per call are all-or-nothing (≥`k` or absent, else the call is rejected); `min_providers` is the popcount. The current snapshot's bits are adjusted in place when a virtual's snapshot changes, and `min_providers` is clamped to the layout size whenever the layout shrinks (removal, smaller re-snapshot), with an event. Signatures of a pending replacement's set are accepted only to activate it ([Changing a live agreement's member set](#changing-a-live-agreements-member-set)); activation is rejected if the new set would push the layout past `MaxPrimarySlots`. |
| `extend_agreement` (virtual) | Rejected unless every snapshotted member is still in the live set; otherwise the owner creates a replacement (base mechanism, [impl doc](./scalable-web3-storage-implementation.md#replacement-agreements)). Also rejected if the re-snapshotted set would push the bucket's slot layout past `MaxPrimarySlots`. |
| `Challenge` | No virtual-specific fields. Any member of the challenged agreement's snapshot may respond; who goes first is decided off-chain ([Who answers](#who-answers)). No membership freeze — liability is fixed by the agreement snapshot. |
| `extend_challenge` | Base extrinsic, permissionless, mirroring `extend_checkpoint`: adds verified signatures over the challenged payload from the snapshotted set to an open challenge's liable set. Only adds accountability. |
| Response transaction extension | Base change ([impl doc](./scalable-web3-storage-implementation.md#response-transaction-extension)). For a virtual account the eligible responders are the challenged agreement's snapshotted members. Duplicate responses are dropped at pool import and never charged, so members need no coordination to avoid racing. |
| `respond_to_challenge` / `ChallengeSlashed` | On failure, slash every signer of the challenged bundle (or the checkpoint's set bits in the virtual's slots) by the snapshotted `per_provider_stake`; event lists them. After any slash, move to `Leaving` any member of a virtual's live set whose remaining stake `< per_provider_stake` (a direct-agreement slash zeroes its stake → all its virtuals), setting `k' = min(k, n')` and `stake' = k' * per_provider_stake`. At `n = 1` the member stays and `per_provider_stake`/`stake` are rewritten to its remaining stake instead, folding `higher_pps_lock` with `max`. Bump `last_stake_event` on the slashed provider and on every virtual in its `ProviderVirtuals`. On a valid response, add the responder's non-reimbursed share to `response_cost_borne` in its entry. |
| `ProviderVirtuals` map | New `StorageMap<AccountId, BoundedVec<VirtualMembership<T>, MaxVirtualsPerProvider>>`: per virtual the member is `Active` in or `Leaving` from, with its `response_cost_borne`. Created by `join_virtual`; leave/kick/auto-removal set `Leaving { until: virtual.cur_until, stake_at_risk }`; `Active` entries removed by `dissolve_virtual`, `Leaving` ones pruned once `until` passed. Read on the slash path (cascade, `last_stake_event`), by `set_stake` and `deregister_provider`, and off-chain for the response order. A side map, not an inline `ProviderInfo` field, so it stays out of every provider's `MaxEncodedLen` and the cap is raiseable. |
| `set_stake` / `deregister_provider` | Base extrinsics gain a check over the caller's `ProviderVirtuals` (≤2 entries): `set_stake` may not lower below any `Active` virtual's `per_provider_stake` (or its unexpired `higher_pps_lock`) nor any unexpired `Leaving.stake_at_risk`; `deregister_provider` is rejected while any such entry exists. Expired `Leaving` entries are pruned on the way. |
| Config constants | `MaxPhysicalMembers` (4: two full virtuals fit in `MaxPrimarySlots`; defence is 4-way, while signing at `k = 3` tolerates one member down — the same as `n = 3`; two-fault signing would need `n = 5` and 16 slots), `MaxVirtualsPerProvider` (2, see [Members](#members-are-registered-physical-providers)), `MaxCoordChannelLen` (`coordination_channel` is `BoundedVec`, not `String`). |
| Runtime API | `provider_type` + virtual composition in `ProviderInfoResponse` ([Discovery](#discovery)). `provider_agreements` additionally returns each virtual agreement's snapshotted member set and `per_provider_stake`, so a client can assess its own exposure after a `last_stake_event` bump from state alone. |
| Member stake release | A leaver's `stake_at_risk` stays locked until its `Leaving.until` — the virtual's `cur_until` when it left, an O(1) upper bound on the expiry of every agreement naming it ([Membership Governance](#membership-governance)). Then the entry is pruned and the stake is free; no announcement window. |

---

## Non-Solutions Considered

**Enforce encryption instead.**

1. Not meaningfully enforceable at the protocol level.
2. Would not even solve the problem: a provider can be ordered to take down *encrypted* content too.
3. Enforcing encryption could read as willful blindness to a court and *increase* liability.
