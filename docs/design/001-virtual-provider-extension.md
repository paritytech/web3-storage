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

A **virtual provider** is a first-class provider identity backed by up to `MaxPhysicalMembers` **physical providers** (its *members*). No chain-enforced minimum: a single provider creates it and it simply does not accept agreements until enough co-members have joined. It has:

- a **synthetic account** (`AccountId` derived from a `PalletId` + a virtual-provider id, like the treasury account), so every existing `AccountId`-keyed extrinsic, agreement, challenge, and runtime-API query addresses it unchanged;
- a **`per_provider_stake`** slice each member puts at risk out of its own physical stake;
- an advertised **`stake`** (the normal provider stake field) — the backing behind any one commitment.

The two figures define `k = stake / per_provider_stake`. `stake` must be a multiple of `per_provider_stake`, so the advertised figure is exactly `k` slices. A commitment is valid only if at least `k` members signed it, and a failed challenge slashes every signer by `per_provider_stake`, so the total slash is at least `stake`. The pallet enforces that `k` is a **strict majority** (`k > members.len() / 2`), so any two valid bundles (sets of at least `k` member signatures) share a signer ([Stake and Slashing](#stake-and-slashing)).

The live `k` applies in two places:

- the calls that set an agreement's `k` from the live settings: the three calls that redeem a quote, `extend_agreement` and `create_replacement`;
- the strict-majority invariant.

Every verification against an agreement uses the `k` of the agreement's last snapshot (impl doc `VirtualSnapshot`). An in-place extension never raises that `k` ([Changing a live agreement's member set](#changing-a-live-agreements-member-set)).

The synthetic account is the only thing clients and buckets reference; members are internal. It never holds a balance: every payout goes from escrow straight to the snapshotted members ([Payment](#payment)), slashes come out of the members' holds, and the deposit for the virtual's records is held on the creator. A bucket lists it in `primary_providers`; in the checkpoint layout it expands to one slot per member ([Checkpoints](#checkpoints)).

A virtual provider and all its members live on one chain: every mechanism here — signature verification, the slash cascade, the stake checks — reads the members' `ProviderInfo` on the chain the virtual is registered on. A provider operating on several chains registers on each, as a physical provider does; those are separate identities with separate stakes.

**Liability lives in the agreement, not in membership.** As the base design snapshots stake into each agreement (price is prepaid and needs no snapshot), a virtual agreement additionally snapshots **the member set and `per_provider_stake` in force when it was struck** (`StorageAgreement.virtual_snapshot: VirtualSnapshot`, and the base `agreement_id`). A commitment binds to its `agreement_id` ([base "Storage Agreements"](./scalable-web3-storage.md#storage-agreements)), hence to a fixed, known member set. So members join and leave freely: leaving the live `members` list only affects *future* agreements — a member stays liable through every agreement whose snapshot names it, until that agreement expires. This is the ordinary base contract ("no early exit; liable until your agreements expire") applied per member, and it removes any need to freeze membership or reason about who saw which off-chain commitment.

A virtual provider has **no address of its own** — clients reach it through members. The discriminant rides `ProviderInfo`'s endpoint field:

```rust
enum ProviderEndpoint<T: Config> {
    /// A single real provider: its own multiaddr. Today's behaviour.
    Physical(BoundedVec<u8, T::MaxMultiaddrLength>),
    /// A virtual provider: no address of its own. The `VirtualProvider` payload
    /// lives in the `VirtualProviders` side map, keyed by the same account;
    /// member multiaddrs are read from each member's own `Providers[m].endpoint`.
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
    /// it until then (see "Stake and Slashing").
    higher_pps_lock: Option<(BalanceOf<T>, BlockNumberFor<T>)>,
    /// The advertised backing is the base `ProviderInfo.stake`. The pallet
    /// does not store the live `k = stake / per_provider_stake`.
    /// Current backing set — the members of *new* agreements, all registered
    /// Physical providers. Live agreements keep the set they snapshotted.
    members: BoundedVec<T::AccountId, T::MaxPhysicalMembers>,
    /// Off-chain coordination endpoint (chat/group) for members to agree on
    /// settings, replacements, desired stake and pricing, ...
    coordination_channel: BoundedVec<u8, T::MaxCoordChannelLen>,
    /// Incremented by every governance change; approval signatures cover it,
    /// so an approval cannot be replayed (see "Membership Governance").
    governance_nonce: u32,
    /// Creator; pays the records deposit (see "Dissolution" for its release).
    depositor: T::AccountId,
}

/// Entry of `ProviderVirtuals[member]`: one per virtual provider the member is
/// in or has left. It locks the member's stake while it is active. An
/// inactive entry stays until the member's next `create_virtual`,
/// `join_virtual`, `set_stake` or `deregister_provider` prunes it; dissolution
/// removes `Active` entries (see "Membership Governance" and "Dissolution").
struct VirtualMembership<T: Config> {
    virtual_provider: T::AccountId,
    state: MembershipState<T>,
    /// Sum of past `Active` stretches in this virtual: `now - since` is folded
    /// in when the member leaves, nothing accrues while `Leaving`, and a rejoin
    /// starts a new stretch. Governance weight is `seniority + (now - since)`
    /// while `Active` (see "Membership Governance").
    seniority: BlockNumberFor<T>,
    /// Sum of the non-reimbursed challenge response fees this member has paid
    /// for this virtual provider. Advisory: drives the off-chain response order
    /// and kick decisions (see "Who answers").
    response_cost_borne: BalanceOf<T>,
    /// Storage deposit for this entry, held on the member and released to it
    /// when the entry is removed. A rejoin keeps the entry and its deposit.
    /// Footprint: the entry's maximum encoded length.
    deposit: T::StorageDeposit,
}

enum MembershipState<T: Config> {
    /// In the live set since `since`; liable at the virtual's current
    /// `per_provider_stake`, or `higher_pps_lock` while that is active and
    /// higher.
    Active { since: BlockNumberFor<T> },
    /// Left the live set (leave, kick or auto-removal); liable through the
    /// agreements that snapshot it, at `stake_at_risk` each, until `until`.
    Leaving { until: BlockNumberFor<T>, stake_at_risk: BalanceOf<T> },
}

/// `VirtualProvider` payload, keyed by the virtual's synthetic account.
#[pallet::storage]
pub type VirtualProviders<T: Config> =
    StorageMap<_, Blake2_128Concat, T::AccountId, VirtualProvider<T>>;

/// Per physical provider: its `VirtualMembership` entries (see "On-Chain
/// Changes").
#[pallet::storage]
pub type ProviderVirtuals<T: Config> = StorageMap<
    _,
    Blake2_128Concat, T::AccountId,
    BoundedVec<VirtualMembership<T>, T::MaxVirtualsPerProvider>,
    ValueQuery,
>;

/// Id of the next virtual provider; never reused.
#[pallet::storage]
pub type NextVirtualId<T: Config> = StorageValue<_, u64, ValueQuery>;
```

The `VirtualProvider` payload is a side map, not an inline `ProviderInfo` enum variant, so it does not enter every provider's `MaxEncodedLen`. `ProviderInfo` stays in the `Providers` map — `Physical` is the untouched default, the change additive.

### Members are registered physical providers

Each member is an ordinary `Physical` provider. Two rules:

- **`Physical` only** — no virtual-in-virtual; joins reject a `Virtual` candidate (recursion makes slashing/stake lookup unbounded and hides the real backing).
- **Both roles, up to `MaxVirtualsPerProvider` virtuals** — a provider can serve direct agreements *and* belong to virtual providers on the same stake. A virtual failure slashes only `per_provider_stake`, so a member's other virtuals may remain intact, whereas a *direct-agreement* failure slashes its whole stake and drops it from all of them. A provider is therefore self-incentivized to keep its direct-agreement risk low once it joins a virtual provider.

  The cap is 2. That covers the concrete use case: a member moves from one virtual provider to another without downtime by belonging to both during the transition, and serves a bucket as a primary through at most one of them ([Checkpoints](#checkpoints)). The cap also bounds the slash cascade: slashing a member walks every virtual it belongs to, so the weight of `resolve_expired_challenge` grows with the cap.

A member need not accept any direct business: it sets `accepting_primary = false` and `replica_sync_price = None` on its own `ProviderInfo` and discovery skips it. The virtual provider's own `ProviderSettings` — price, `accepting_primary`, replica price — are set through `set_virtual_settings` and are independent of the members'.

---

## Stake and Slashing

**A failed challenge slashes every signer of the challenged commitment by `per_provider_stake`.** Every member has the same exposure and the same loss, regardless of its total stake. A valid bundle has at least `k` signers, so the total slash is at least `k * per_provider_stake`, which equals the advertised `stake`. `stake` is therefore the minimum backing behind any commitment. All members sign what they store ([Commitments](#commitments-threshold-signatures)), so with `n` members the usual backing is `n * per_provider_stake`.

The challenged commitment identifies its signers: it names its `agreement_id`, and that agreement's snapshot fixes the eligible set. The bundle names which eligible members signed; for a checkpoint, the set bits in the slot layout name them. The challenge copies the signers into `virtual_liability.liable` at creation. A challenger could present only `k` of the signatures it holds and leave the other signers off the liable set. To prevent that, **`extend_challenge`** lets anyone add further signatures over the same payload from the snapshotted set while an off-chain challenge is open. It mirrors `extend_checkpoint`. A checkpoint challenge does not need it. A signer facing the slash adds its co-signers with `extend_challenge`, so every signer ends up slashed.

**Liability is enforced on members, not on the virtual account.** The invariant: for every live agreement, each member it names holds at least the `per_provider_stake` that agreement snapshotted. A slash is the one exception: a member slashed elsewhere may then hold less than it owes; a slash saturates at what the member has, and `last_stake_event` reports it (below). Three pieces enforce it in O(1), without walking agreements:

- **The virtual tracks two dates.** `cur_until`: the max expiry over its agreements and pending replacements (bumped at `create_replacement`, base "Changeable Stake"). It is the base field, never reset here because the pallet writes the virtual's `stake` only from `create_virtual` and `set_virtual_settings`, without the `set_stake` bookkeeping. `higher_pps_lock`: after a lowering of `per_provider_stake`, the old value and `cur_until` at that time — the highest value a live agreement may still carry, and until when.
- **Each member's `ProviderVirtuals` entry says what it owes that virtual, the floor for `set_stake`** ([Membership Governance](#membership-governance)). `Active`: the virtual's live `per_provider_stake`, or `higher_pps_lock` while it is active and higher. `Leaving { until, stake_at_risk }`: a fixed amount until a fixed block, both copied from the virtual at the moment it left. Every lock follows the base `lock_active` rule. `higher_pps_lock` and every member's entry for a virtual are active until their time has passed and the virtual has no open challenge (`pending_challenges == 0`). So a challenged virtual provider locks the stake of all its members, including members outside the challenged snapshot and leavers past `until`. This is accepted to keep the check O(1) without per-member counters; a third party can extend a leaver's lock only by paying for challenges against the virtual's live agreements.
- **Two base extrinsics read the entries.** `set_stake` may not lower a member's stake below what any entry says it owes; `deregister_provider` is rejected while any entry is `Active` or an active `Leaving`, and while the member's own `pending_challenges > 0`.

The virtual's own `stake` holds nothing and needs no lock. Changing `per_provider_stake` through `set_virtual_settings`: raising auto-removes every live member whose stake does not cover the new value, as a slash does ([below](#stake-and-slashing)) — the seniority majority decides what the slice is, and only members that hold it stay; lowering writes `higher_pps_lock = (old, cur_until)` and is rejected while a previous lock is active (in particular while the virtual is challenged) — the base stake-lowering rule, applied to the figure members are actually bound to.

**`k` is a strict majority.** Any two valid bundles then share a signer, so the group cannot split into two disjoint sets producing conflicting commitments or checkpoints — each backed by the full advertised `stake`, which only one of them can cover. Governance has its own threshold ([Membership Governance](#membership-governance)).

**Slashing cascades to a member's other backings.** A member's stake is shared across everything it backs. Losing `per_provider_stake` here lowers what remains; if the remainder falls below another virtual's live `per_provider_stake`, the member can no longer back new agreements there and is **auto-removed from that virtual's live `members`** (existing ones keep their snapshot and the member's residual liability). The threshold is the live value, not an active `higher_pps_lock`: that lock only blocks voluntary lowering through `set_stake`; a slash reduces what backs old agreements either way, and a member that still covers the live slice keeps serving new ones. A member slashed for its *own* direct agreement loses its whole stake and is auto-removed from all its virtuals. The cascade *is* the team-vetting incentive: a member bears the risk of whom it pools with.

Auto-removal — after a slash or a raise of `per_provider_stake` — changes membership only; `k` and `stake` stay. For the member's record it counts as a leave, not a kick: the group did not reject it, it stopped holding the slice. If it leaves `n < k`, the virtual stops accepting agreements until the seniority majority admits a member or lowers `stake` ([Membership Governance](#membership-governance)) — unless it is empty, which is terminal. The removed member's `ProviderVirtuals` entry turns `Leaving`; it stays bound to the agreements that name it.

**An empty virtual is terminal.** Removal has no exception, so every live member always holds at least the slice, and the last member can go too — slashed out or leaving. A virtual with no members accepts no agreements and takes no joins; its live agreements run to expiry on their snapshots and the members' `Leaving` entries. Once all its agreements are settled and no challenge is open (`pending_challenges == 0`), anyone may dissolve it ([Dissolution](#dissolution)).

**Clients learn about a slash from provider state, not (only) the event log.** Three cases bump `ProviderInfo.last_stake_event`: a slash of the provider itself; a failed challenge against a virtual provider; a slash elsewhere that leaves a member of a virtual with less stake than its `ProviderVirtuals` entry for that virtual owes. A client returning after arbitrary downtime walks its own agreements, reads each provider's field, and compares against the last block it checked, bounded by its agreement count. The entry contains the highest amount owed to the virtual, so a bump can still leave an agreement with a lower snapshot fully backed. To determine actual exposure the client reads the agreement's snapshotted signer set from `AgreementResponse` ([On-Chain Changes](#on-chain-changes)) and those members' current stake. A member slashed elsewhere leaves live agreements naming it under-backed relative to their snapshot even when the virtual's own advertised `stake` is unchanged — which is why the bump is not conditional on `stake` moving.

A member complying with a takedown is protected only if another member serves the chunk; if none does, the signers of the challenged commitment are slashed (it may be one of them) — the extension cannot let content vanish for free. A complying member remains visible off-chain: its `response_cost_borne` stops growing while the others' does. Whether the group tolerates that is a member decision.

### Capacity

Not partitioned, and not aggregated across roles. A virtual agreement's bytes count into the **virtual provider's** `committed_bytes`; a member's own counter is unaffected by what the virtuals it belongs to commit to. Nothing is enforced against either figure — the base enforces no stake-per-byte requirement at all ([base "Stake vs. capacity"](./scalable-web3-storage-implementation.md#stake-vs-capacity)) — so aggregating them would spend weight on every member's counter at each virtual agreement create, expire and auto-removal for no gain. A client wanting a physical provider's total adds its direct `committed_bytes` and those of the virtuals it belongs to, read off `ProviderVirtuals`.

---

## Commitments: Threshold Signatures

Today a commitment is one provider signature over a `CommitmentPayload`, verified against that provider's `public_key`. A virtual commitment is a **bundle of ≥`k` member signatures** over the same payload, where `k` is the agreement's `k` ([Model](#model)). Each signature must verify against its member's key. Only a bundle meeting `k` is a valid commitment for the virtual provider.

`k` is the validity minimum, not a target. **All members sign every commitment they store** — signing is the service, and a member that does not is the freeloader [Payment](#payment) lets the others kick. The bundle carries every signature collected and every signer is liable, so `stake = k * per_provider_stake` is the floor on backing and `n * per_provider_stake` the norm; `k` is what the group still guarantees with `n - k` members down.

This is the key-theft mitigation: one stolen member key produces one signature, below `k`, so it cannot mint a fraudulent commitment.

### Write path

The base "immediate guarantee from one signature" ([design doc](./scalable-web3-storage.md#the-chain-as-credible-threat)) becomes a short collection round:

1. Client uploads to one member — the **coordinator** for this write. (A virtual provider is a replication set, so the data reaches *all* members regardless of which one is picked.)
2. The coordinator passes the data and the client's `/commit` request to the other members of the named agreement's snapshot. Each member appends the request's `data_roots` to the `base` the client named and signs the new `CommitmentPayload` naming the request's `agreement_id`: the live agreement's, or its pending successor's for a replacement ([Changing a live agreement's member set](#changing-a-live-agreements-member-set)).
3. After signing, each member sends its signature to every other member, the coordinator included, so that every signer holds its co-signers' signatures ([Stake and Slashing](#stake-and-slashing)). Once the coordinator holds ≥`k` signatures, it returns the **bundle** (every signature it holds) to the client. The bundle is the client's guarantee; the client verifies it against the commitment it computed itself (impl doc ["Commit"](./scalable-web3-storage-implementation.md#commit), writer steps). How long the coordinator waits past `k` for slow members is a group setting: waiting adds latency and backing.

**If collection stalls** (member down, or coordinator withholds): a sub-`k` bundle is not a commitment, so the client simply retries via a *different* coordinator (data is idempotent). A coordinator can withhold but not forge — every signature is checked against member keys by client and chain.

### Quotes

A quote (`AgreementTerms`) binds the provider to a new agreement. For a virtual provider, a quote therefore needs the group's consent in the same form as a commitment: at least `k` signatures of its live `members`. The new agreement snapshots those `members`. `terms.provider` names the virtual's synthetic account. The member serving `/negotiate` collects the signatures before responding. One stolen key cannot commit the group to an agreement.

For the same reason, every base call that takes one provider's signature takes a `ProviderSignatures` set: one entry for a physical provider, at least `k` members for a virtual one. Checkpoints and `extend_challenge` take flat signature pairs instead ([impl doc "Provider Public Key & Signature Type"](./scalable-web3-storage-implementation.md#provider-public-key--signature-type)).

### Checkpoints

The checkpoint path does not distinguish virtual from physical. `checkpoint` and `extend_checkpoint` use the base signature format: `(u8, Signature)` pairs, where the `u8` is a slot index. The client collects the signatures from its physical primaries directly, and from a coordinator for a virtual primary.

The pallet derives the bucket's **slot layout** and does not store it. The layout is `primary_providers` expanded in order:

- a physical primary is one slot;
- a virtual primary is its agreement's snapshotted members, in snapshot order.

Bit `i` of `primary_signers` means slot `i` signed. An entry's index refers to a slot of the bucket's current slot layout. In `checkpoint` it can also refer to a **pending slot**: the pending slots follow the current slots, one per member of each pending successor's snapshot (index space and checks: impl doc `checkpoint`). `primary_signers` has no bits for pending slots. Each entry signs over its slot's `agreement_id`. One physical account belongs to at most one primary of a bucket, counting each primary's slots and its pending successor. A violation fails with `DuplicateSigner`; impl doc `BucketSnapshot.primary_signers` lists the calls this applies to and how to get past it. Verification checks each entry's signature against its slot account's key. After a failed checkpoint challenge, the pallet slashes a physical primary's whole stake, or, for a virtual, each member in `virtual_liability.liable` (the set bits, copied at challenge creation) by the snapshotted `per_provider_stake`. The checkpoint path has no bundle type and no per-slot bitmask.

One rule is virtual-specific: **per virtual primary, a submission is all-or-nothing**. In `checkpoint`, a virtual's signatures must be either absent or at least `k` (its agreement's `k`; in pending slots, the successor's `k`); otherwise the pallet rejects the call. `extend_checkpoint` accepts either a whole group with ≥`k` signatures or further members of a group that already has ≥`k` set bits in the current snapshot. It never accumulates signatures toward `k`. A bit is therefore set only when its group has reached `k`, and a set bit means liable while its slot's agreement is live.

`min_providers` counts **set bits**, the signed slots. A client with one 4-member virtual sets `min_providers` in member slots. A value at or below `k` has no effect, because a checkpoint that includes the virtual already has at least `k` set bits. A value above `k` is a real extra requirement.

`MaxPrimarySlots` (8) bounds the expanded layout, the bitfield and the signatures verified per checkpoint. With `MaxPhysicalMembers = 4`, these layouts fit:

- two full virtuals with disjoint member sets (the migration case);
- one virtual plus four physical primaries;
- eight physical primaries.
- ..

The layout grows only when a primary is added or when a replacement with a larger set activates. The pallet checks `MaxPrimarySlots` when the primary is added and when the replacement is created, counting a primary with a pending successor at the larger of its current slots and the successor's members, so an activation never exceeds it (impl doc `MaxPrimarySlots`). An activating `checkpoint` ([Changing a live agreement's member set](#changing-a-live-agreements-member-set)) needs a bucket admin (`NotBucketAdmin`). It builds `primary_signers` for the resulting layout from its own signatures and counts them against `min_providers` (impl doc `checkpoint`).

---

## Challenges

A challenge targets the **virtual account** through the existing `challenge_checkpoint`, `challenge_offchain` and `challenge_replica` calls, unchanged. The challenge resolves against the **member set snapshotted in the challenged `agreement_id`** and copies that set at creation (`Challenge.virtual_liability`). The set of liable members is fixed when the agreement is struck, and later membership changes do not affect it, so no membership freeze is needed. On failure, every signer of the challenged commitment loses `per_provider_stake`. **Any snapshotted member may respond**. The pallet slashes the group only if no member responds. Members that joined later are not eligible: they are not in the snapshot, cannot sign for the agreement, and hold none of its data.

### Who answers

The chain assigns no duty. It records one figure per member and virtual provider, `response_cost_borne` in the member's `ProviderVirtuals` entry: the sum of the non-reimbursed shares of the response fees it has paid for this virtual provider (known at resolution, since the pallet computes the reimbursement). Members use it to decide, off-chain, who goes first.

**The convention (provider-node default).** On `ChallengeCreated`, every snapshotted member computes the same **candidate order** from state at the creation block:

- the order contains the snapshotted set;
- `Active` members come first, then members in `Leaving`;
- the order sorts each of the two groups by `response_cost_borne`, ascending; a member with no entry counts as 0;
- position in the snapshot breaks ties.

The order covers the whole snapshot, not only `liable`. A challenge tests whether the virtual provider can serve the data, and any member that holds the data proves that; the chain accepts a response from any snapshotted member. A liable-only order would let a challenger decide who answers: for `challenge_offchain` it picks `liable`, so it could single out one member to pay, or present `k` members that are offline and get them slashed while the group holds the data. Leavers remain liable and eligible, so the order is never empty. Leavers go last because the group can no longer kick them.

Candidate `i` submits its response `i · L` anchor blocks after creation (`L = 2`). It does not submit if the challenge is already resolved, or if a response for the challenge is already pending in its node's transaction pool. The node finds a pending response with `author_pendingExtrinsics`, decoded for `respond_to_challenge` on that `challenge_id`. The pool check is an optimisation: the pool rejects a duplicate at import for free, so the check only saves the later responder its bandwidth. The member that has paid least goes first, and whoever pays moves back for the next challenge. The order is fair in expectation and needs no coordination round and no clock in the protocol itself.

**Why members follow it.** Responding costs the responder `c(t)`, the provider-borne share of the response fee, rising from 10% to 50% with latency (base cost table). Not responding costs nothing while another member does. Nobody responding costs every signer `per_provider_stake`, orders of magnitude more. So a member behind the others cannot gain by waiting: either it still ends up answering, at higher `c(t)`, or another member answers and its lag — visible on-chain to the peers who can kick it — grows. A member ahead of the others gains nothing by answering early; it only pays. Given a credible kick threat, which the kick mechanism assumes anyway, the order is self-enforcing, and the slash asymmetry guarantees that some member answers before the deadline whatever the others do. Deviation is harmless: a member that races anyway pays more and drops back; one that never answers falls behind and is kicked.

The kick is the only enforcement, and a member holding more than half of the seniority weight cannot be kicked ([Membership Governance](#membership-governance)). Such a member can stop signing and responding and still be paid for every agreement that snapshots it. The other members then either accept that and keep the virtual's record/reputation, or leave and form their own virtual. A buy-out of a member's seniority and share is a possible later addition.

**Nobody can be targeted.** The order derives from the cost figures, and a challenger can move them only by paying for a real response. A challenger can read who is candidate 0 and challenge at that moment. That member's figure then rises, and it stops being candidate 0. Each member therefore answers about 1/`n` of the costly challenges. A member cannot inflate its own figure by challenging its own virtual provider. The pallet rejects a challenge by a member of the virtual's live set or of the agreement's snapshot (`SelfChallenge`; impl doc ["Extrinsics"](./scalable-web3-storage-implementation.md#extrinsics), comment "Who may challenge"). From another account, inflating its own figure costs ~100% of a response fee to gain ~10%.

**Public challenges** are reimbursed in full, so they add 0 to the figure and need no special case. The order still applies, to avoid duplicate work; among members with equal figures the tie-break is snapshot position, so the first such member answers every fully reimbursed challenge, at no cost to it.

**Duplicate responses cost nothing.** The base transaction extension for raced calls ([impl doc](./scalable-web3-storage-implementation.md#transaction-extension-for-raced-calls)) gives every response to the same challenge the same `provides` tag, so a second one is rejected at pool import like a duplicate nonce, pruned from every pool once the first is included, and never charged. `L` therefore need not guarantee ordering — it only keeps honest members from wasting bandwidth — and a collision is not an error.

### Cost split

Unchanged from base ([Challenge Game](./scalable-web3-storage.md#the-challenge-game)): the virtual account is the `provider`; tiering and the ≥50% floor apply. The responding member pays its response fee and is reimbursed from the challenger's deposit exactly as a physical provider is. Whatever it bears stays with it and is added to its `response_cost_borne`; the response order, not redistribution, shares the cost. The agreement's payment is no source for it — it settles at expiry and may be burned entirely.

### Replicas

A virtual provider can hold replica agreements like any provider. Its liability as a replica is its last confirmed sync, so `confirm_replica_sync` takes the same group consent: at least `k` signatures of the agreement's snapshot over `ReplicaSyncPayload`, submitted by any snapshotted member. Collection follows the write-path round ([Write path](#write-path)), started by a member instead of the client. Any member may act as coordinator: it proposes the `roots` it holds, headed by the current tip, collects signatures — a member that has not synced to that state yet signs once it has — and submits at ≥`k`. Concurrent rounds — another coordinator proposing an older root and finishing first — are harmless: the pool keeps one sync per agreement and drops the rest uncharged (impl doc ["Transaction extension for raced calls"](./scalable-web3-storage-implementation.md#transaction-extension-for-raced-calls)), and the further-advancing one is submitted after it, once `min_sync_interval` has elapsed. The accepted signers are stored with the sync in `last_sync_signers` (a bitmask over the snapshot, like a checkpoint's slots), and a failed `challenge_replica` against that sync slashes them; any snapshotted member may respond. The per-sync `sync_price` is paid on each confirmed sync, equally to the snapshotted members, as every payout is ([Payment](#payment)). A member-set change is handled by a replacement, as for a primary ([Changing a live agreement's member set](#changing-a-live-agreements-member-set)); a replica successor activates by the first `confirm_replica_sync` with `activate_replacement` set signed by ≥`k` of its snapshot, confirming a root no older than the old set's last confirmed sync (base "Replacement agreements").

### Residual key-theft surface

Threshold commitments close the forged-commitment path. What one stolen member key can still do, all short of a slash:

- **not respond** → another snapshotted member answers;
- **forge a replica sync** → only for that member's *own* replica agreements; a virtual provider's sync needs `k` signatures;
- **trigger governance** → needs a seniority majority ([Membership Governance](#membership-governance)).

So a stolen key can degrade service but cannot forge a commitment. An invalid challenge response is rejected and the challenge stays open — only a timeout slashes (base "Verification") — so garbage from a stolen key costs the group a transaction fee, not a slash ([#473](https://github.com/paritytech/web3-storage/issues/473)).

---

## Payment

Each agreement's payment stays in escrow until the agreement settles, as for a physical provider, and is then paid **equally to the members it snapshotted**, directly from escrow — natural, since they store the same data and risk the same `per_provider_stake`. The synthetic account is never involved: the split needs the agreement's snapshot, which the settling call has. Up to `MaxPhysicalMembers` transfers per settled agreement; the rounding remainder goes to the first member in snapshot order. Agreement statistics and lifetime revenue are recorded on the virtual's own `ProviderInfo`, not on the members'. Challenge-response fees are not redistributed here — they stay with the member that paid them, and the response order evens them out over time ([Who answers](#who-answers)).

A **refund** (base `refund_agreement`) gives the snapshotted members' payment back to the client, so it needs a seniority majority of the current members; members that left lose their share without a vote — the one exception to "payment follows the snapshot" below; self-limiting, since the approving members forfeit their own shares too. It is the group's way out when a snapshotted member will not sign and the group cannot deliver, where the replacement flow is the client's.

Payment follows the snapshot, period: a member named in an agreement is paid for it whether it later left, was kicked, or was auto-removed — it carried the liability. Kicking a **freeloader** (never signs / never responds) therefore changes nothing already struck; it only keeps the member out of future agreements. A kick cannot take money already struck. The remaining signal is reputational — a kicked-count on the provider's own record, so other groups can see it before admitting it; its exact shape is open. If a non-cooperating member causes commitments to fail, the group can refund the client to avoid a burn; nobody is paid in that case.

---

## Membership Governance

Changes to the **live `members`** set do not change existing agreements, which keep their snapshot ([Model](#model)). After such a change, while the live set differs from an existing agreement's snapshot, the owner can extend that agreement only through a replacement ([Changing a live agreement's member set](#changing-a-live-agreements-member-set)). Membership can change freely. The only invariant is that `k` stays a strict majority of `members.len()`, which the pallet checks on joins and settings changes. **Agreements are rejected while `members.len() < k`**, because the group cannot deliver its advertised `stake`. That state is normal right after creation. It is also how independent providers find each other: an open virtual below `k` advertises the slice and the target it wants filled.

**Two thresholds.** Liability is by count: a commitment needs `k` signatures, one slice each, and a newcomer's slice counts the same as a veteran's. Governance — settings, join, kick, refund, extension blocking, dissolve — is by **seniority**.

- **Weight.** A member's weight is the number of anchor blocks it has been `Active` in this virtual. It is stored in the member's `ProviderVirtuals` entry and frozen while `Leaving`, so a member that leaves and rejoins keeps it, as long as the entry still exists.
- **Decision.** A change passes when its approvers hold more than half of the current members' total weight. Two disjoint sets cannot both hold more than half, so conflicting decisions cannot both pass.
- **Approvals.** Members sign `blake2_256(GOVERNANCE_CONTEXT | genesis_hash | SCALE(virtual account, governance_nonce, change))`, with `GOVERNANCE_CONTEXT = "virtual-governance-v1:"`. The virtual account in the payload keeps an approval from being replayed against another virtual the member is in; the nonce keeps it from being replayed against the same one; the genesis hash keeps it from being replayed on another chain (impl doc "Chain binding" under `CommitmentPayload`). The provider nodes collect the signatures, as they do for commitments, and any member submits them with the call, except `join_virtual`, which the candidate submits; the pallet adds up the signers' weight. The `coordination_channel` is where the operators discuss changes; an operator app with a chat is planned as the default.
- **Why seniority.** A newcomer has no weight until it has been a member for as long as the others, so nobody can join a valuable virtual and block its decisions.
- **Effect over time.** A creator holds all the weight while alone and loses control as others accrue. From three members on, the group outweighs it once their combined tenure exceeds the creator's. In a pair the first member stays dominant; a pair needs some tie-break, and the junior can always leave.

**Trust model.** A virtual provider is a small DAO. The protocol enforces what client or chain can check: commitments, slashing and payment. Everything else between members is the group's business: who stores, who signs, who withholds data. Members establish working relationships and may check each other subjectively; a member can, for example, fetch data from a peer and see whether the peer fetches it from the requesting member's node to answer. Nobody can prove such observations to others, so disputes are settled by governance, as in any partnership. This is sound because misplaced trust has a bounded cost and a member can end it. A member that stores what it signs can answer every challenge on it itself (any snapshotted member may respond, [Who answers](#who-answers)), so a failing partner cannot get it slashed. A failing partner can still cost it:

- response fees (`response_cost_borne`) and a share of the work;
- its payment share: the owner can burn the payment at settlement, and a partner holding a seniority majority can approve a refund ([Payment](#payment));
- locked stake: while the virtual is challenged, the member cannot lower its stake below what it owes or deregister ([Stake and Slashing](#stake-and-slashing)).

The group decides whom to admit, and any member can leave alone at any time ([Leaving](#leaving)). Leaving ends the member's exposure for future agreements; it stays liable through the agreements that name it until they expire.

**A member leaving bumps the provider's `version`** (base [Term Pinning](./scalable-web3-storage-implementation.md#term-pinning-no-surprise-agreements)). Composition is part of what a client buys: `3`-of-`4` is more resilient than `3`-of-`3` even at identical `k` and `stake` (one more member can go dark before the group can't cover). So a departure is a worse-terms change a client may have declined, and its pinned request/extension correctly fails. A join (more redundancy, strictly better) does not bump. `set_virtual_settings` bumps when it lowers `stake` or raises `k` (fewer members may be down before the group cannot sign); other settings changes do not.

Two liabilities to separate:

- **Signing new agreements** — needs a live set of `>= k`.
- **A leaver's residual liability** — a member that left the live set is still liable through every agreement whose snapshot names it, until that agreement expires. Its `ProviderVirtuals` entry turns `Leaving { until, stake_at_risk }` instead of being removed. `until` is the virtual's `cur_until` at that moment: an upper bound on the expiry of every agreement naming it, since all were struck while it was live, an agreement naming a member that has left cannot be extended in place, and a pending replacement naming it already bumped `cur_until` to the successor's expiry when it was created ([Changing a live agreement's member set](#changing-a-live-agreements-member-set)). So no challenge naming it can be created after `until`; open ones keep the entry active while the virtual is challenged (its `pending_challenges`, base "Changeable Stake"; for a dissolved virtual see [Dissolution](#dissolution)). `stake_at_risk` is the highest `per_provider_stake` those agreements may carry: the live value, or `higher_pps_lock` if active and higher. For a member removed by a raise of `per_provider_stake`, the live value is the one before the raise. The entry blocks `set_stake` below `stake_at_risk` and `deregister_provider` while it is active (`until` not passed, or the virtual is challenged); once it is no longer active it is pruned by the next `create_virtual`, `join_virtual`, `set_stake` or `deregister_provider` of that member. O(1) to create, O(1) to check. The bound is conservative — it also covers agreements struck before the leaver joined — which is the price of not walking agreements; the figure is public before joining. A `Leaving` entry keeps its `MaxVirtualsPerProvider` slot until it is pruned.

### Create / join / kick

- **Create:** a `Physical` provider calls `create_virtual` with `stake`, `per_provider_stake` and the coordination channel. The call requires:
  - `stake` a multiple of `per_provider_stake`;
  - `stake >= MinProviderStake`, as for any provider;
  - `per_provider_stake > 0`.

  `set_virtual_settings` enforces the same rules. `create_virtual` also applies to the creator the checks `join_virtual` applies to a candidate: `Providers[creator].stake >= per_provider_stake`, and fewer than `MaxVirtualsPerProvider` `ProviderVirtuals` entries after pruning its inactive `Leaving` entries. The pallet then:
  - derives the synthetic account;
  - registers a `Virtual` `ProviderInfo`;
  - writes the caller as sole member;
  - holds the records deposit on the creator: a storage deposit for the maximum encoded length of the virtual's records (the `VirtualProviders` entry and the synthetic `ProviderInfo`). A creator that leaves before [Dissolution](#dissolution) can settle the deposit with the remaining members off-chain. The creator also pays the deposit for its own `ProviderVirtuals` element, like every member (`VirtualMembership.deposit`).
- **Join** (`join_virtual`): the candidate submits the call, with a seniority-majority approval. The candidate's consent is its signature on the extrinsic. The candidate pays only the storage deposit for its own `ProviderVirtuals` element (`VirtualMembership.deposit`). Candidate must be `Physical` with `stake >= per_provider_stake`, below `MaxVirtualsPerProvider` (inactive `Leaving` entries pruned first); rejected if it would make `k` a non-majority. A rejoin resumes the candidate's kept seniority.
- **Kick** (seniority majority; the target's weight counts in the total, so a member holding more than half cannot be kicked): moves the member to `Leaving`. No challenge freeze — a kicked member keeps its residual liability, so nothing is shed.

### Leaving

A member can always leave, alone, with `leave_virtual` — the one membership change that needs no approval, so nobody is held hostage. It moves to `Leaving`; `k` and `stake` stay, and if the group is now under `k` it stops accepting until it recruits or lowers `stake`; if the leaver was the last member, the virtual is empty and terminal. The leaver signs no new agreements from then on (quotes need the live members). For every agreement whose snapshot names it, it keeps signing commitments, checkpoints and syncs and stays liable until that agreement expires; its stake also stays locked while the virtual is challenged. Because clients contracted with the *virtual* account, this churn never breaks the client-facing "data stays until expiry".

### Changing a live agreement's member set

`extend_agreement` re-snapshots current terms under the same `agreement_id`. Off-chain commitments already signed for that id must keep verifying without the client re-checking them, so for a virtual an in-place extension follows two rules:

- **Same member set:** the live set must contain exactly the snapshotted members: no member left and no member joined. Otherwise the call fails with `VirtualMembersChanged`. The extension keeps the snapshot's `members` as stored, in their order, and re-snapshots only `stake` and `per_provider_stake`. A dropped member would void every commitment it signed. A joined member would change the eligible set, the slot layout and the positions that `primary_signers` and `last_sync_signers` index, with no new `agreement_id` to show the change.
- **`k` does not rise:** the live `stake / per_provider_stake` must not exceed the agreement's `k`. Otherwise the call fails with `VirtualThresholdRaised`. A higher `k` would make a bundle signed by the old `k` members fall below the new `k` and stop verifying. An equal or lower `k` keeps every existing bundle valid. Old and new bundles still share a signer, because the new `k` is still a strict majority of the same set.

When an extension breaks either rule, the client uses a **replacement**, which has a fresh `agreement_id`. If the live set has fewer than `k` members, the replacement fails too (`VirtualBelowThreshold`; impl doc `create_replacement`), and the agreement runs to expiry unless the group gets back to `k`.

A replacement is the base mechanism ([impl doc](./scalable-web3-storage-implementation.md#replacement-agreements)), here with a new member set:

1. The owner creates the replacement: a pending successor stored in the agreement record. The successor contains:
   - a fresh `agreement_id`;
   - the current member set, `stake` and `per_provider_stake`, so it has its own `k`;
   - expiry `old.expires_at + additional_duration`;
   - the payment for `additional_duration` at the current price, locked.

   Creation bumps the virtual's `cur_until` to the successor's expiry. As a result, a later lowering of `per_provider_stake` writes a `higher_pps_lock` that lasts until the successor's expiry, and a member that leaves while the successor is pending gets a `Leaving.until` of at least the successor's expiry.

   `additional_duration` may be 0, for a pure member swap. The two in-place rules above do not apply. The extension gates apply to it, also at zero duration, and it needs no new quote; impl doc [`create_replacement`](./scalable-web3-storage-implementation.md#extrinsics) lists every rejection. An owner can make the members sync for nothing; the members can stop accepting extensions, or block them on that owner's agreements (`set_extensions_blocked`).

   While the successor is pending, the old agreement cannot be extended, topped up or transferred (impl doc `create_replacement` in ["Extrinsics"](./scalable-web3-storage-implementation.md#extrinsics)). The owner cannot cancel the successor either, because the new set may already have synced and signed. The successor ends only by activation or together with the old agreement. The successor is not live for data: it cannot be challenged, commitments naming it are not yet valid, and it has no slots in the bucket's layout. The members' stake is bound from creation. The old agreement continues and remains fully challengeable.
2. The new members sync the data and sign. **Activation requires the old agreement to be live.**
   - **Primary agreement:** a member without the bucket's history catches up by committing from the empty MMR at the snapshot's `start_seq` (impl doc ["Commit"](./scalable-web3-storage-implementation.md#commit), steps 2 and 5). The client sends the catch-up `/commit`, naming the successor's `agreement_id`, to a member of the successor's snapshot, which passes the request to the other members of that snapshot ([Write path](#write-path)). The client uploads no data: a successor member fetches what it lacks from the nodes of the old and the successor snapshots (impl doc ["Commit"](./scalable-web3-storage-implementation.md#commit), step 3). If fewer than `k` successor members get the data, the successor cannot activate and lapses with the old agreement (step 4). The first `checkpoint` that carries ≥`k` of the new members' signatures in the successor's pending slots **activates** the replacement. That call contains no signatures in the old set's slots (impl doc `checkpoint`). This is a fresh checkpoint submitted by a bucket admin (impl doc `checkpoint`), never `extend_checkpoint`. The admin attests the latest state, off-chain commitments included, and the new set proves it holds that state. Activation voids every commitment under the old `agreement_id`, including off-chain ones the activating checkpoint does not contain; for the writers' duty, see impl doc ["Replacement agreements"](./scalable-web3-storage-implementation.md#replacement-agreements). An admin that never submits the activating checkpoint makes the new set sync for nothing. In the activation block the record becomes the new agreement, with the new id, the new snapshot and the fixed `expires_at`. The new set is then liable for the latest state. The guarantee is continuous for the state the activating checkpoint contains.
   - **Replica agreement:** the first `confirm_replica_sync` with `activate_replacement` set that carries ≥`k` of the new members' signatures activates the replacement. The confirmed commitment must advance over the old set's last confirmed sync or be identical to it, so no new checkpoint is needed. The successor keeps the old agreement's `sync_balance` and `min_sync_interval` (base "Replacement agreements").

   Activation writes nothing to the members' entries: the `cur_until` bump at creation already gave every member that left while the successor was pending a `Leaving.until` of at least the successor's expiry (step 1).
3. The old agreement ends there with extension semantics: its elapsed period is paid to its snapshotted members, equal split, leavers included; the unelapsed remainder moves into the successor's escrow, where the payment for `additional_duration` already is. The new set serves the rest of the old stretch at the old price and the added stretch at the new one; the owner pays once for every block ([impl doc "Replacement agreements"](./scalable-web3-storage-implementation.md#replacement-agreements)).
4. If the old agreement expires first, the successor never activated: the old set is paid for the whole term it served, the successor's payment is released to the owner, and the new set gets nothing — it was never challengeable and served nothing; its sync work is its business risk. This is the graceful failure when the new set does not sign.

So a client starts a swap early enough for the new set to sync before the old agreement expires. Only `checkpoint` (primary) and `confirm_replica_sync` (replica) ever verify signatures against a pending set, and only to activate it.

### Dissolution

`dissolve_virtual` succeeds once all the virtual's agreements are settled (`committed_bytes == 0`) and no challenge is open (`pending_challenges == 0`). A seniority majority decides; anyone can dissolve an empty virtual. Dissolution:

- removes the `VirtualProviders` entry and the synthetic `ProviderInfo`;
- releases the records deposit to the creator, even if the creator has left;
- removes the live members' `Active` entries and releases their deposits.

The leavers' `Leaving` entries remain, because the virtual's record lists only live members. Each is inactive from then on, whatever its `until`: no agreement of the virtual names the leaver any more, and no challenge is open or can be created. (`until` can be later than the last expiry, because a lapsed replacement successor does not lower `cur_until`.) The pallet recognizes such an entry by the missing `VirtualProviders` record (`NextVirtualId` never reuses an id) and prunes it like any other inactive `Leaving` entry ([Membership Governance](#membership-governance)).

---

## Role of Multiple Primaries

Multiple primaries become less important. A client that wants high stake for an important bucket usually picks one virtual primary, which is already decentralized internally, instead of combining several physical primaries itself. Multiple primaries remain useful where the client wants direct control:

- zero-downtime migration: run two virtual providers with disjoint member sets during the switch;
- existing trust relationships: a provider the client knows or runs.

---

## Discovery

Clients select on **stake**, unchanged — virtual-ness is not a selection axis (a low-stake virtual provider is no better than a physical one of equal stake). Because high stake gives a provider strong reason to pick independent backers, the highest-stake providers will tend to be virtual, so the decentralization dividend comes for free from chasing stake. Discovery therefore just makes a virtual provider's stake legible and exposes its internals. Additive changes:

- `ProviderInfoResponse` gains a physical/virtual discriminant. A virtual provider already reports its `stake` in the existing field (so stake sorting/matching works unchanged); it adds `per_provider_stake`, `k` and `member_count`.
- It lists the member accounts: clients reach a virtual provider through its members' endpoints ([Model](#model)). 
- Discovery ranks providers off-chain (base impl doc "Runtime API"); a virtual provider needs no special handling there — it competes on its `stake` like any other.

---

## On-Chain Changes

Concrete additions the base pallet needs. All additive — `Physical` behaviour is unchanged.

| Area | Change |
|---|---|
| `ProviderInfo` | `multiaddr` becomes `endpoint: ProviderEndpoint<T>` (`Physical(multiaddr)` \| `Virtual`) — the tag is the discriminant. A virtual provider's base `stake` field holds `k * per_provider_stake`, written only by `create_virtual` and `set_virtual_settings`, never by `set_stake`. New `last_stake_event: Option<BlockNumberFor<T>>` (see [Stake and Slashing](#stake-and-slashing)). For a virtual provider, the base `deposit` field contains the ticket for the records deposit ([Create](#create--join--kick)). |
| `VirtualProviders` map | New `StorageMap<AccountId, VirtualProvider<T>>` (`per_provider_stake`, `higher_pps_lock`, member accounts, coordination channel, `governance_nonce`, `depositor`), keyed by the synthetic account; loaded only when members are needed. Invariants: `stake` a multiple of `per_provider_stake`, `k` a strict majority of `members.len()`. The pallet writes the virtual's `stake` field only from `create_virtual` and `set_virtual_settings`, without the base `set_stake` bookkeeping, so its `cur_until` is never reset and is the max expiry over its live agreements and pending replacements. |
| `StorageAgreement` (virtual) | `virtual_snapshot: Option<VirtualSnapshot>`: the **member set** and `per_provider_stake` in force at creation or extension. `None` for a physical provider. The base already snapshots `agreement_id` and stake. Liability and the set of eligible responders resolve against this snapshot, not the live set. |
| Synthetic account | `PalletId` + virtual-provider id (treasury-style), so all `AccountId`-keyed extrinsics/queries work unchanged. |
| `NextVirtualId` | New `StorageValue<u64>`: a monotone counter of virtual-provider ids. Ids are never reused. `create_virtual` takes the next id and derives the synthetic account from it. |
| New extrinsics | `create_virtual`, `join_virtual`, `leave_virtual` (the leaver alone), `kick_member`, `set_virtual_settings`, `dissolve_virtual`. Each needs approval by a seniority majority ([Membership Governance](#membership-governance)), except `create_virtual`, `leave_virtual` and the dissolution of an empty virtual. `join_virtual` is rejected for an empty virtual. `set_virtual_settings` takes `stake` and `per_provider_stake` together. Lowering `per_provider_stake` sets `higher_pps_lock = (old, cur_until)` and is rejected while a previous lock is active. Raising `per_provider_stake` moves every live member whose stake is below the new value to `Leaving`, recorded as a leave, not a kick. `set_virtual_settings` bumps `version` when `stake` decreases or `k` increases (base "Term Pinning"). The pallet enforces `per_provider_stake > 0`, `stake` a multiple of `per_provider_stake`, `k` a strict majority of `members.len()`, `Providers[m].stake >= per_provider_stake` per member, and `MaxVirtualsPerProvider` on the creator and on a candidate, after pruning inactive `Leaving` entries. It rejects a `Virtual` creator or candidate. |
| Agreement creation (virtual) | Rejected while `members.len() < k` (`VirtualBelowThreshold`). `create_replacement` follows the same rule. |
| Provider signatures | Every call that takes one provider's signature takes the base `ProviderSignatures` set. Exceptions: `checkpoint` and `extend_checkpoint` take flat `(u8, Signature)` slot pairs, and `extend_challenge` takes flat `(AccountId, Signature)` pairs. For a virtual account the set contains at least `k` distinct members, each verified against its own key. The member set depends on the call. The three calls that redeem a quote (`create_bucket_with_primary`, `add_primary_provider`, `add_replica_provider`) use the live `members`. `challenge_offchain` (commitments) and `confirm_replica_sync` use the agreement's snapshot. `confirm_replica_sync` with `activate_replacement` set, and a `checkpoint` entry in a pending slot, use the pending successor's snapshot. |
| `confirm_replica_sync` (virtual) | Callable by any snapshotted member (`provider` names the virtual). Stores the accepted signers in `last_sync_signers`; a failed `challenge_replica` slashes them. With `activate_replacement` set, verified against the pending successor's snapshot, the call activates the replacement instead. |
| `checkpoint` / `extend_checkpoint` | Base signature format: `(u8, Signature)` pairs keyed by slot index. The slots derive from `primary_providers`, with a virtual expanded to its snapshotted members. An entry's index refers to the current slot layout (`InvalidSlot`, `DuplicateSlot`; impl doc `checkpoint`). `checkpoint` also accepts pending slots, one per member of each pending successor's snapshot ([Checkpoints](#checkpoints)). A virtual's signatures in `checkpoint` are all-or-nothing; `extend_checkpoint` can also add further members of a group that already reached `k` ([Checkpoints](#checkpoints)). `min_providers` counts the set bits. When a primary is added or removed, the pallet adjusts the current snapshot's bits in place; an activating `checkpoint` builds them for the new layout. A group in the pending slots of a primary's successor activates the replacement; the call then needs a bucket admin (`NotBucketAdmin`) and rejects signatures in that primary's current slots (`ConflictingSigners`). A replica successor activates in `confirm_replica_sync` ([Changing a live agreement's member set](#changing-a-live-agreements-member-set)). |
| `refund_agreement` (virtual) | Base extrinsic; for a virtual account approved by a seniority majority of its current members ([Payment](#payment)). |
| `set_extensions_blocked` / `claim_expired_agreement` (virtual) | `set_extensions_blocked`: base extrinsic; for a virtual account approved by a seniority majority of its current members, as `refund_agreement`. `claim_expired_agreement`: anyone may call it after `SettlementTimeout` (base), so an agreement of a virtual, which has no key, still settles. |
| `extend_agreement` (virtual) | Rejected if the live member set differs from the snapshot (`VirtualMembersChanged`), or if the extension would raise the agreement's `k` (`VirtualThresholdRaised`). In both cases the owner creates a replacement instead. An accepted extension keeps the snapshot's `members` and re-snapshots `stake` and `per_provider_stake` ([Changing a live agreement's member set](#changing-a-live-agreements-member-set)). |
| `Challenge` | Base field `virtual_liability: Option<VirtualLiability>`, `None` for a physical provider. It contains `snapshot`, the challenged agreement's `VirtualSnapshot`, and `liable`, a bitmask over `snapshot.members` of the members liable for the challenged commitment. The challenge copies both at creation. The challenge needs no agreement record, so the agreement may be settled, extended or replaced while the challenge is open. Later changes to checkpoint bits or `last_sync_signers` do not affect the challenge. Creation increments the virtual's `pending_challenges`, which locks its members' stake (Stake and Slashing). Any member of the challenge's `virtual_liability.snapshot` may respond. The members decide off-chain who goes first ([Who answers](#who-answers)). |
| `extend_challenge` | Base extrinsic, permissionless, mirroring `extend_checkpoint`. It adds verified signatures over the challenged payload from the snapshotted set to an open off-chain challenge's `virtual_liability.liable`. It can only add accountability. |
| Transaction extension for raced calls | Base change ([impl doc](./scalable-web3-storage-implementation.md#transaction-extension-for-raced-calls)). For a virtual account, the eligible responders are the members in the challenge's `virtual_liability.snapshot`. The eligible submitters of `confirm_replica_sync` are the members of the selected snapshot. The pool drops duplicate responses and duplicate sync confirmations at import, so they are never included and cost no fee. Members therefore need no coordination to avoid racing. |
| `respond_to_challenge` / `resolve_expired_challenge` / `ChallengeSlashed` | On timeout (`resolve_expired_challenge`), slash every member in the challenge's `virtual_liability.liable` by the snapshotted `per_provider_stake`. `liable` contains the accepted bundle signers, the checkpoint's set bits, or a replica sync's `last_sync_signers`. After any slash, move to `Leaving` every member of a virtual's live set whose remaining stake is `< per_provider_stake`. The comparison uses the live value; `higher_pps_lock` only gates `set_stake`. A direct-agreement slash zeroes the member's stake, so the member moves to `Leaving` in all its virtuals. `k` and `stake` stay. Removing the last member leaves the virtual empty and terminal. Bump `last_stake_event` on the slashed provider and on the challenged virtual. Bump it on each other virtual in the slashed member's `ProviderVirtuals` only if the member's remaining stake is below what the member's entry for that virtual owes. That is always the case after a direct-agreement slash. On a valid response, add the responder's non-reimbursed share to `response_cost_borne` in its entry. |
| `ProviderVirtuals` map | New `StorageMap<AccountId, BoundedVec<VirtualMembership<T>, MaxVirtualsPerProvider>>`: per virtual the member is `Active` in or `Leaving` from, with its `response_cost_borne` and seniority. Each element carries its own storage deposit (`VirtualMembership.deposit`). Created by `create_virtual` (the creator's element) and `join_virtual`; leave/kick/auto-removal set `Leaving { until: virtual.cur_until, stake_at_risk }` and freeze seniority; a rejoin flips it back to `Active` and resumes it; `Active` entries removed by `dissolve_virtual`, `Leaving` ones pruned once they are no longer active (`until` passed and the virtual not challenged, or the virtual dissolved; see [Dissolution](#dissolution)). Read on the slash path (cascade, `last_stake_event`), off-chain for the response order, and by `create_virtual`, `join_virtual`, `set_stake` and `deregister_provider`, which prune the `Leaving` entries that are no longer active. A side map, not an inline `ProviderInfo` field, so it stays out of every provider's `MaxEncodedLen` and the cap is raiseable. |
| `set_stake` / `deregister_provider` | Base extrinsics gain a check over the caller's `ProviderVirtuals` (≤2 entries): `set_stake` may not lower below any `Active` virtual's `per_provider_stake` (or its active `higher_pps_lock`) nor any active `Leaving.stake_at_risk`; `deregister_provider` is rejected while any such entry exists or the member's `pending_challenges > 0`. `Leaving` entries that are no longer active are pruned on the way. |
| Config constants | `MaxPhysicalMembers` (4), `MaxVirtualsPerProvider` (2, see [Members](#members-are-registered-physical-providers)), `MaxCoordChannelLen` (`coordination_channel` is `BoundedVec`, not `String`). Why `MaxPhysicalMembers = 4`: two full virtuals with disjoint member sets fit in `MaxPrimarySlots`. Any of the 4 members can defend a challenge. Signing at `k = 3` tolerates one member down, the same as `n = 3`. Signing with two members down would need `n = 5`, and two such virtuals would need more than 8 slots. |
| Runtime API | `provider_type` + virtual composition in `ProviderInfoResponse` ([Discovery](#discovery)). `AgreementResponse` (`agreement_info`, `bucket_agreements`, `provider_agreements`) additionally contains a virtual agreement's snapshotted member set and `per_provider_stake`, so a client can assess its own exposure after a `last_stake_event` bump from state alone. |
| Events | `VirtualCreated`, `VirtualMemberJoined`, `VirtualMemberLeft` (by `leave_virtual`, or auto-removed after a slash or a raise of `per_provider_stake`), `VirtualMemberKicked`, `VirtualSettingsChanged`, `VirtualDissolved`. Base `ChallengeExtended` reports a change of `virtual_liability.liable`. |
| Member stake release | A leaver's `stake_at_risk` stays locked until its `Leaving.until` — the virtual's `cur_until` when it left (which already covers any pending replacement naming it), an O(1) upper bound on the expiry of every agreement naming it — and while the virtual is challenged ([Membership Governance](#membership-governance)). Then the entry is pruned and the stake is free; no announcement window. |
