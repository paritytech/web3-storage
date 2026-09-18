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

From the two, `k = ceil(stake / per_provider_stake)` — the signers needed so their slices cover the advertised stake; a commitment is valid only if signed by ≥`k` members, and a failed challenge slashes exactly those `k` by `per_provider_stake`. The pallet enforces that this derived `k` is a **strict majority** (`k > members.len() / 2`): a minority must never be able to bind the group ([Stake and Slashing](#stake-and-slashing)). So `stake` cannot be set so low (relative to `per_provider_stake` and member count) that `k` falls to half or below.

The synthetic account is the only thing clients and buckets reference; members are internal. A bucket lists it in `primary_providers` (one of the ≤5 slots).

**Lability lives in the agreement, not in membership.** As the base design now snapshots price and stake into each agreement, a virtual agreement additionally snapshots **the member set and `per_provider_stake` in force when it was struck** (and the base `agreement_id`). A commitment binds to its `agreement_id` ([base "Storage Agreements"](./scalable-web3-storage.md#storage-agreements)), hence to a fixed, known member set. So members join and leave freely: leaving the live `members` list only affects *future* agreements — a member stays liable through every agreement whose snapshot names it, until that agreement ends. This is the ordinary base contract ("no early exit; liable until your agreements expire") applied per member, and it removes any need to freeze membership or reason about who saw which off-chain commitment.

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
    /// Advertised backing lives in the base `ProviderInfo.stake`. Together with
    /// `per_provider_stake` it yields `k = ceil(stake / per_provider_stake)`
    /// (not stored). Invariant: `k` a strict majority of `members.len()`.
    /// Current backing set — the members of *new* agreements. Live agreements
    /// keep the set they snapshotted (see below).
    members: BoundedVec<Member<T>, T::MaxPhysicalMembers>,
    /// Off-chain coordination endpoint (chat/group) for members to agree on
    /// settings, replacements, desired stake and pricing, ...
    coordination_channel: BoundedVec<u8, T::MaxCoordChannelLen>,
}

struct Member<T: Config> {
    account: T::AccountId,   // a registered Physical provider (never Virtual)
    /// Duty/quality counters — advisory, drive off-chain member decisions.
    missed_challenges: u32,  // was on duty, did not respond
    covers: u32,             // responded while not on duty (helped out)
}
```

The `VirtualProvider` payload is a side map, not an inline `ProviderInfo` enum variant, so it does not enter every provider's `MaxEncodedLen`. `ProviderInfo` stays in the `Providers` map — `Physical` is the untouched default, the change additive.

### Members are registered physical providers

Each member is an ordinary `Physical` provider. Two rules:

- **`Physical` only** — no virtual-in-virtual; joins reject a `Virtual` candidate (recursion makes slashing/stake lookup unbounded and hides the real backing).
- **Both roles, any number of virtuals, no cap** — a provider can serve direct agreements *and* belong to several virtual providers on the same stake. Safer than it sounds: a virtual failure slashes only `per_provider_stake`, so a member's other virtuals usually survive it, whereas a *direct-agreement* failure slashes its whole stake and drops it from all of them. A provider is therefore self-incentivized to keep its direct-agreement risk low once it joins a virtual provider — no cap needed.

---

## Stake and Slashing

**A failed challenge slashes each of the `k` signers of the challenged commitment by `per_provider_stake`.** Uniform — same duty, same loss, regardless of a member's total stake. Total slashed = `k * per_provider_stake` = the advertised `stake`, so the client's number is exactly the backing behind the commitment. The signers are known: the commitment names its `agreement_id`, whose snapshot fixes the eligible set, and the bundle names which of them signed.

**`k` is a strict majority.** Any two valid bundles then share a signer, so the group cannot split into two disjoint sets producing conflicting commitments, checkpoints or governance decisions — each backed by the full advertised `stake`, which only one of them can cover.

**Slashing cascades to a member's other backings, shown to clients as a stake drop.** A member's stake is shared across everything it backs. Losing `per_provider_stake` here lowers what remains; if the remainder falls below another virtual's `per_provider_stake`, the member can no longer cover that slice and is **auto-removed from that virtual's live `members`** (affecting only new agreements — existing ones keep their snapshot and the member's residual liability). A member slashed for its *own* direct agreement loses its whole stake and is auto-removed from all its virtuals. Clients simply see the virtual provider's advertised stake fall. The cascade *is* the team-vetting incentive: a member bears the risk of whom it pools with.

A member complying with a takedown is protected only if another member serves the chunk; if none does, the signers of the challenged commitment are slashed (it may be one of them) — the extension cannot let content vanish for free. A complying member is still identifiable off-chain via its duty counters (it missed no duty it was excused from).

### Capacity

Not partitioned. A member's bytes — direct or virtual — count once into its own `committed_bytes`, checked against its own stake by the base `committed_bytes * MinStakePerByte` invariant ([base "Stake vs. capacity"](./scalable-web3-storage-implementation.md)), so membership consumes ordinary capacity through the ordinary check. No virtual-specific capacity accounting.

---

## Commitments: Threshold Signatures

Today a commitment is one provider signature over a `CommitmentPayload`, verified against that provider's `public_key`. A virtual commitment is a **bundle of ≥`k` member signatures** over the same payload, each verified against the respective member's key (`k` from [Model](#model)). Only a bundle meeting `k` is a valid commitment for the virtual provider — and it is thereby backed by ≥ the advertised virtual stake.

This is the key-theft mitigation: one stolen member key produces one signature, below `k`, so it cannot mint a fraudulent commitment.

### Write path

The base "immediate guarantee from one signature" ([design doc](./scalable-web3-storage.md#the-chain-as-credible-threat)) becomes a short collection round:

1. Client uploads to one member — the **coordinator** for this write. (A virtual provider is a replication set, so the data reaches *all* members regardless of which one is picked.)
2. The coordinator fans the data out and collects signatures over the new `CommitmentPayload` (which names the virtual provider's `agreement_id`).
3. At ≥`k` signatures it returns the **bundle** — the client's guarantee.

**If collection stalls** (member down, or coordinator withholds): a sub-`k` bundle is not a commitment, so the client simply retries via a *different* coordinator (data is idempotent). A coordinator can withhold but not forge — every signature is checked against member keys by client and chain. Last resort: a plain physical agreement with one member — weaker, always available.

### Checkpoints

In `bucket.primary_providers` the virtual provider is **one** account, so it occupies **one** bit in the `primary_signers` bitfield and counts as **one** toward `min_providers`. Its bit is set by presenting a valid `k`-signature bundle rather than a single signature. Concretely, `checkpoint` / `extend_checkpoint` accept, for a virtual account, that bundle in place of the single `(AccountId, Signature)` entry (see [On-Chain Changes](#on-chain-changes)). A bucket can still mix a virtual provider with a few physical primaries within the ≤5 slot budget.

The internal member count is **not** exposed as extra primary slots, so a client reading `bucket.primary_providers.len()` must not read redundancy directly from it — the runtime API surfaces the virtual composition separately ([Discovery](#discovery)).

---

## Challenges

A challenge targets the **virtual account** (existing `challenge_checkpoint` / `challenge_offchain` / `challenge_replica`, unchanged). It resolves against the **member set snapshotted in the challenged `agreement_id`**, so who is liable is fixed and unaffected by later membership churn — no freeze needed. On failure the `k` signers are each slashed `per_provider_stake`. **Any member may respond** at any point in the window; the group is slashed only if none does. Two questions remain: **who is expected to answer** (duty) and how covering is kept from becoming a stalemate.

### Duty and covering

Duty assigns an *expected* responder so the group shares the work and a challenger cannot single out one member by timing. It rotates over the challenged agreement's snapshotted set:

```
duty_index = ((creation_block + total_defended) / dispute_window_len) % member_count
```

evaluated once at creation and snapshotted into the challenge (with `member_count`), like the base `Challenge.authorized` — deterministic for the whole window. Duty never grants exclusivity (covering is always open; a duty-only-after-split rule would strand covering against public challengers, who get no split). It only drives advisory, off-chain-consumed counters: on-duty non-response → `missed_challenges += 1`; off-duty response → `covers += 1`.

**Fallback against the volunteer's dilemma:** during a grace period the on-duty member is expected; after it, a deterministic second member (`(duty_index + 1) % member_count`); then any member. Two named responders before the free-for-all.

Duty may land on a member that lacks the chunk (members are expected to replicate all data; a `missed_challenges` bump records the lapse). Safe, because any holder covers immediately — duty is work-attribution, not liability. Liability is the snapshotted signer set; defense is open to any holder.

### Cost split

Unchanged from base ([Challenge Game](./scalable-web3-storage.md#the-challenge-game)): the virtual account is the `provider`; tiering and the ≥50% floor apply. The responding member pays its response fee and is reimbursed from the challenger's deposit exactly as a physical provider is; any residual it bore is then taken off the top of the challenged agreement's payment before the equal [Payment](#payment) split, so the fee does not land on it alone.

### Residual key-theft surface

Threshold commitments close the forged-commitment path. What one stolen member key can still do, all short of a slash:

- **miss a duty** → `missed_challenges` bump; any other member covers;
- **forge a replica sync** → hits only that member's *own* replica agreements, not the `k`-signed virtual commitments;
- **trigger governance** → needs `k`-of-members ([Membership Governance](#membership-governance)).

So a stolen key can degrade service but cannot slash — the concrete gain over a lone provider.

---

## Payment

Each agreement's payment accrues to the synthetic account and is split **equally among the members it snapshotted** — natural, since they store the same data and risk the same `per_provider_stake`. A member that fronted a challenge-response fee for that agreement is reimbursed off the top before the split, so the fee does not land on it alone.

Members can **kick a freeloader** (never signs / never covers) from the live set before its next agreement settles, so it earns nothing further. The reverse — a majority kicking an honest but redundant member before payout — removes no availability (it was, by definition, covered) and is deterred by an on-chain kick counter; if it ever matters, a vesting payout can be added. A business risk, not a protocol break.

---

## Membership Governance

Changes to the **live `members`** set affect only *future* agreements — existing agreements keep their snapshot ([Model](#model)). So membership churns freely; the only invariant is that the live set can still sign: **`members.len() >= k`** with `k = ceil(stake / per_provider_stake)` a strict majority. All changes are **`k`-of-members authorized** (never a single key). `stake` and `per_provider_stake` (hence `k`) may be adjusted in the same call as a membership change, so the set is never momentarily under-`k` or below majority.

**A member leaving bumps the provider's `version`** (base [Term Pinning](./scalable-web3-storage-implementation.md)). Composition is part of what a client buys: `3`-of-`5` is more resilient than `3`-of-`4` even at identical `k` and `stake` (one more member can go dark before the group can't cover). So a departure — whether or not it also lowers `stake` — is a worse-terms change a client may have declined, and its pinned request/extension correctly fails. A join (more redundancy, strictly better) does not bump. This is in fact the sharpest reason `version` exists: nothing else captures a composition change.

Two liabilities to separate:

- **Signing new agreements** — needs a live set of `>= k`.
- **A leaver's residual liability** — a member that left the live set is still liable through every agreement whose snapshot names it, until that agreement ends. Its `per_provider_stake` is at risk that whole time; its stake unlocks (base `set_stake` / deregister rules) only once no snapshotting agreement remains. This is the ordinary base "liable until your agreements expire", per member — nothing virtual-specific.

### Create / join / kick

- **Create:** a founder (`Physical`) calls `create_virtual` with `stake` and `per_provider_stake`; the pallet derives the synthetic account, writes the `VirtualProvider` (founder as sole member), registers a `Virtual` `ProviderInfo`, not accepting. It starts accepting once `members.len() >= k`.
- **Join** (`join_virtual`): candidate must be `Physical` with `stake >= per_provider_stake`. While bootstrapping (not yet accepting, `members.len() < k`) the founder approves joins; once operational, joins are `k`-approved like other changes.
- **Kick** (`k`-approved): drops a member from the live set; allowed while `members.len()` stays `>= k` (lower `stake`, hence `k`, in the same call if needed). No challenge freeze — a kicked member keeps its residual liability, so nothing is shed.

### Leaving

A member can always leave the *signing rotation*; how easily depends on whether its slice is still needed to reach `stake` (it keeps serving its snapshotted agreements either way):

1. **Not needed** (the remaining slices still reach `k`) → leave immediately, no coordination.
2. **Needed but `stake` is lowerable now** → lower `stake` (dropping `k`) and leave.
3. **Needed and `stake` can't be lowered yet** (base stake-lock: a higher generation still owed) → no clean leave, but **force your way out** exactly as a physical provider does: stop accepting agreements and extensions, drain to expiry. Members wanting continuity are incentivized to help (find a replacement, lower `stake` when possible).

Since stake rarely changes, case 1 is the common one — usually a member can just go. It is never helpless (its signature is required for every change, so it has leverage), and never worse off than a lone provider: it drops out of signing immediately but stays bound to what it already backs until expiry. Because clients contracted with the *virtual* account, this churn never breaks the client-facing "data stays until expiry".

### Changing a live agreement's member set

A client that wants a *live* agreement's backing set changed (e.g. swap a member) uses the base extension mechanism, which re-snapshots current terms — here, the current `members`. The client drives it (it knows the tip), and because the base contract forbids leaving it uncovered the swap is a two-phase replace, not an in-place edit:

1. The old agreement enters **PhasingOut**: it stops taking new commitments but stays fully challengeable, so the client keeps its guarantee.
2. A new agreement (new `agreement_id`, new member set) sits **PendingActivation** with no slashing risk. Its new members sync the data, then sign; the client submits `checkpoint` with their `k`-signature bundle at the current tip.
3. That checkpoint activates the new agreement and deletes the old atomically.
4. If the old agreement reaches normal expiry first, the new one never activated: its members were never at risk and the client's extra payment is refunded.

Case 4 is the graceful failure if the new set won't sync/sign (e.g. members declining the extension) — marginally worse than a normal extension, never unsafe. So a client should start a swap **early enough to transfer data before the old agreement expires**. This reuses the base `agreement_id` + snapshot machinery; the only virtual-specific part is that "current terms" includes the member set.

### Dissolution

`k`-approved once no agreement snapshots any member (all expired) and no challenge is open: removes the `VirtualProviders` entry and the synthetic `ProviderInfo`. No funds move — nothing was escrowed.

---

## Role of Multiple Primaries

Multiple primaries lose importance: a client chasing stake for an important bucket usually lands on one virtual primary that already carries decentralization internally, rather than hand-assembling physical ones. They stay useful where the client wants direct control — zero-downtime migration (run two virtual providers during the switch), or existing trust relationships (a provider it knows or runs).

---

## Discovery

Clients select on **stake**, unchanged — virtual-ness is not a selection axis (a low-stake virtual provider is no better than a physical one of equal stake). Because high stake gives a provider strong reason to pick independent backers, the highest-stake providers will tend to be virtual, so the decentralization dividend comes for free from chasing stake. Discovery therefore just makes a virtual provider's stake legible and exposes its internals. Additive changes:

- `ProviderInfoResponse` gains a physical/virtual discriminant. A virtual provider already reports its `stake` in the existing field (so stake sorting/matching works unchanged); it adds `per_provider_stake`, `k`, `member_count`, and the kick counter.
- Member accounts (and voluntary jurisdiction attestations) may optionally be exposed as transparency — granularity is a per-deployment choice, defaulting to count + aggregate (independence is provider-self-attested in the base design anyway).
- `find_matching_providers` needs no virtual-specific scoring — a virtual provider competes on aggregate stake like any other.

---

## On-Chain Changes

Concrete additions the base pallet needs. All additive — `Physical` behaviour is unchanged.

| Area | Change |
|---|---|
| `ProviderInfo` | `multiaddr` becomes `endpoint: ProviderEndpoint<T>` (`Physical(multiaddr)` \| `Virtual`) — the tag is the discriminant. A virtual provider's base `stake` field holds `k * per_provider_stake`, kept in sync on any `k`/`per_provider_stake` change. |
| `VirtualProviders` map | New `StorageMap<AccountId, VirtualProvider<T>>` (`per_provider_stake`, `k`, members, coordination channel), keyed by the synthetic account; loaded only when members are needed. Invariant: `k` a strict majority of `members.len()`. |
| `StorageAgreement` (virtual) | Additionally snapshots the **member set** and `per_provider_stake` in force at creation/extension (base already snapshots `agreement_id`, price, stake). Liability and duty resolve against this snapshot, not the live set. |
| Synthetic account | `PalletId` + virtual-provider id (treasury-style), so all `AccountId`-keyed extrinsics/queries work unchanged. |
| New extrinsics | `create_virtual` (founder), `join_virtual`, `leave_virtual`, `kick_member`, `set_virtual_settings`, `dissolve_virtual` — governance ones `k`-of-members authorized. Enforce `Providers[m].stake >= per_provider_stake` per member, `members.len() >= k`, and reject a `Virtual` candidate. |
| Commitment verification | For a virtual account, verify a **`k`-signature bundle** (signers ∈ the `agreement_id`'s snapshotted set) over `CommitmentPayload` instead of a single signature. |
| `checkpoint` / `extend_checkpoint` | Accept that bundle where the signer is a virtual account; virtual = one bit in `primary_signers`, one toward `min_providers`. |
| `Challenge` | Snapshot `duty_index` and `member_count` at creation (like `authorized`). No membership freeze — liability is fixed by the agreement snapshot. |
| `respond_to_challenge` / `ChallengeSlashed` | On failure, slash the `k` signers by `per_provider_stake`; event lists them. After any slash, auto-remove from a virtual's live set any member whose remaining stake `< per_provider_stake` (a direct-agreement slash zeroes its stake → removed from all). Update `missed_challenges` / `covers`. |
| Config constants | `MaxPhysicalMembers`, `MaxCoordChannelLen` (`coordination_channel` is `BoundedVec`, not `String`). |
| Runtime API | `provider_type` + virtual composition in `ProviderInfoResponse` ([Discovery](#discovery)). |
| Member stake release | A leaver's `per_provider_stake` stays at risk until no agreement it backs is still live — the base "liable while an agreement is active" rule, applied per member. Once none remain it is un-challengeable and its stake frees immediately (no announcement window). |

---

## Non-Solutions Considered

**Enforce encryption instead.**

1. Not meaningfully enforceable at the protocol level.
2. Would not even solve the problem: a provider can be ordered to take down *encrypted* content too.
3. Enforcing encryption could read as willful blindness to a court and *increase* liability.
