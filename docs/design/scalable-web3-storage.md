# Scalable Web3 Storage

| Field | Value |
| --- | --- |
| **Authors** | eskimor |
| **Status** | Draft |
| **Version** | 2.3 |
| **Related** | [Implementation Details](./scalable-web3-storage-implementation.md), [Proof-of-DOT Infrastructure Strategy](https://docs.google.com/document/d/1fNv75FCEBFkFoG__s_Xu10UZd0QsGIE9AKnrouzz-U8/) |

## Version History

| Version | Changes |
|---------|---------|
| 2.3 | Private buckets clarified (visibility flag, Reader role, member-only primary challenges, tier-split challenge stats). **Read**: new "Bucket Visibility & Access" section; "The Challenge Game". |
| 2.2 | Challenge cost model reworked and clarified: a valid response never touches the provider's stake. The challenger's deposit covers the on-chain response cost; authorized challengers (bucket members + agreement owners) get a split where the provider bears a fraction (challenger's share floored at 50%, as leverage—not cheap recovery), while the general public pays in full (anti-DoS, since a provider can't serve everyone equally). Stake is slashed only on a missing/invalid response. |
| 2.1 | Clarification on rewards for the challenger: There should be none, just refund. Plus some corrections with regards to PDP and Filecoin. |
---

## Pitch

Storage isn't free—someone pays, someone naturally cares, we take advantage of
that.

Storage providers lock stake, clients can challenge. If a provider
doesn't serve data, they lose their stake. The chain exists as a credible
threat, not the hot path—normal operations happen directly between clients and
providers. The chain is only needed for setup, checkpoints and disputes.

We don't stop at storage—we go through some lengths to guarantee retrieval too.
Challenges extract actual data on-chain. Too slow and expensive for bulk
recovery, but your most critical data is directly recoverable. More importantly,
answering a challenge means posting the data on-chain—far costlier than serving
the same bytes off-chain—so providers are pressured to serve directly. A
provider's own counterparties make it bear a share of that cost, so it serves
them promptly; strangers fund their challenges in full. A provider's
challenge-cost exposure is thus bounded to the accounts it chose to deal with:
it controls its risk by vetting whom it signs agreements with.

Existing Web3 storage either proves too much (Filecoin's continuous proofs—heavy,
slow, chain-bound) or/and guarantees too little (IPFS—no persistence at all). We use
game theory: Rational providers serve because being challenged costs more
than any savings from deleting data. This scales with provider capacity, not
chain throughput.

Stronger guarantees emerge from data importance. When data matters enough, more
parties add replicas, each with stake at risk. A bucket replicated across major
providers in multiple jurisdictions is practically guaranteed by the combined
economic stakes. And if you don't trust the aggregate: verify yourself, or add
your own replica.

Also mutable storage is provided, in addition to content addressed immutable storage.

---

## Table of Contents

1. [The Problem with Web3 Storage](#the-problem-with-web3-storage)
2. [Our Approach: Pragmatic Verification](#our-approach-pragmatic-verification)
3. [Architecture Overview](#architecture-overview)
4. [Economic Model](#economic-model)
5. [Proof-of-DOT and Read Incentives](#proof-of-dot-and-read-incentives)
6. [Client Strategies](#client-strategies)
7. [Data Model](#data-model)
8. [Use Cases](#use-cases)
9. [Comparison with Existing Solutions](#comparison-with-existing-solutions)
10. [Rollout](#rollout)
11. [Future Directions](#future-directions)

---

## The Problem with Web3 Storage

Decentralized storage faces a fundamental tension: **guarantees vs. throughput**. Existing solutions either prove too
much (expensive, doesn't scale) or guarantee too little (no persistence). But there's a deeper issue: even "strong"
cryptographic proofs don't actually guarantee what users care about.

### IPFS: A Protocol, Not a Storage Solution

IPFS is a content-addressing protocol—a way to name data by its hash. This is genuinely useful: content hashes are
location-independent, self-verifying names. But IPFS is not a storage system. It provides:

- **Naming**: Data identified by hash (CID)
- **Routing**: DHT for peer discovery
- **Transfer**: Protocol for fetching chunks

What IPFS explicitly does *not* provide:

- **Persistence**: No guarantee anyone stores the data
- **Availability**: A CID with no providers is a dangling pointer—~50% of year-old content fails a first retrieval attempt
- **Incentives**: No reason for nodes to serve data to strangers
- **Fast discovery**: retrieval in practice takes seconds; publishing provider records ~16s median

The fundamental issue is that a content hash is just a name. Knowing the name doesn't tell you where the data lives, who
stores it, or whether it exists at all. You must hope someone, somewhere, cares enough to keep it.

### Filecoin: Incentivizing IPFS Storage

Filecoin adds an incentive layer to IPFS. Providers lock collateral and commit to store data. The chain verifies storage
through cryptographic proofs—Proof-of-Replication (data is stored) and Proof-of-Spacetime (data persists over time).

**The heavyweight approach (PoRep/PoSt):**
- Data "sealed" into 32GB sectors (hours: ~3h CPU-bound PC1 plus GPU phases)
- Every sector proven on-chain every 24 hours (WindowPoSt)
- Chain processes millions of proofs daily at network scale

This works for cold archival. But for interactive applications: write latency is
minutes to hours, hardware requirements exclude commodity servers, and 32GB
sectors waste space & cost for small files: This makes Filecoin largely unusable
for end users.

**The lighter approach (PDP, May 2025):**
Filecoin's Proof of Data Possession improves hot storage significantly:
- No sealing required—data immediately accessible
- No GPU needed—CPU-only verification (SHA2 Merkle proofs)
- Constant proof size—160 bytes challenged regardless of data volume
- ~10-20x lower gas costs than WindowPoSt

But PDP still requires scheduled proofs—one per data set per day. At network
scale with millions of data sets, the chain must still process millions of proof
submissions daily. **Chain throughput still bounds storage capacity**, just at a
higher ceiling. Write latency is still bound by chain latency.

### The Deeper Problem: What Do Proofs Actually Guarantee?

Filecoin's proving ensures at a high cost that data is stored, but there is no
guarantee at all that it will also be retrievable. A provider can provide all
the required proofs and still not serve the data at all to anybody (or not at
the necessary capacity).

### CID Addressing Hides Dependencies

Content addressing (CIDs) has a subtle problem: **it obscures where data actually lives**.

When you reference `bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3okurat...`, you're saying "I want data with this
hash." You're not saying:
- Who stores it (by design)
- What guarantees they've made
- How long they've committed to store it
- What stake backs that commitment

This is fine for ephemeral caching. It's problematic for anything you depend on.
You can't easily answer "will this data exist next month?" without tracking down
the storage deals separately or rely on third party indexers.

### Summary

Even with Filecoin, IPFS is still lacking:

| Question | IPFS Answer | Filecoin Answer |
| --- | --- | --- |
| Who stores my data? | Unknown (hope someone) | Provider (but deal expires) |
| How long will it persist? | Unknown | Until deal ends |
| What if they delete it? | Nothing | Slash (but data still gone) |
| How do I verify? | Fetch it yourself | Chain proves periodically |
| Does it scale? | N/A (no guarantees) | Bounded by chain throughput |

**What's missing:** A system that makes storage relationships explicit, scales with provider capacity rather than chain
throughput, and provides guarantees that match what users actually need—not cryptographic proofs for their own sake, but
assurance that data remains accessible.

---

## Our Approach: Pragmatic Verification

We start from a different premise: **make dependencies explicit, verify what matters, skip the overhead for what doesn't.**

### Bucket Addressing: Explicit Storage Relationships

Instead of pure content addressing (CID), we use **bucket addressing**. A bucket is an on-chain entity that makes
storage relationships visible and removes the need for costly DHT lookups - it provides discoverabilty.

**Immutable references** (like CIDs) include a content hash:
```
bucket://42/<data_root>
         │       └── Merkle root of the data (content-addressed, immutable)
         └── bucket_id (on-chain, shows who stores it)
```

This is similar to a CID—the `data_root` pins the exact content—but the bucket tells you *where* to find it and *who* is
responsible for storing it.

**Mutable references** resolve against current bucket state:
```
bucket://42/fs/my_website/logo.png    // filesystem path, latest snapshot
bucket://42/latest/leaf/17            // current leaf 17
```

The key difference from pure CIDs:

| Aspect | CID (`bafybei...`) | Bucket (`bucket://42/...`) |
| --- | --- | --- |
| Who stores it? | Unknown | Providers A, B, C (on-chain) |
| What guarantees? | Unknown | 1000 DOT stake each |
| How long? | Unknown | Until block 1,000,000 |
| Content immutability | Hash = content | Hash in path = immutable; path-only = mutable |
| Can I verify? | Fetch and hope | Fetch & challenge if needed |

**The bucket_id is the stable anchor.** Providers can come and go, but the bucket_id never changes. Applications
reference buckets, not providers. When you see a bucket reference, you can look up exactly who stores it, what stake
backs it, and how long they've committed.

### Storage Only Matters If Someone Cares

Our design assumption: Someone pays for the storage, so someone naturally cares: Storage and retrievability are not
proven cryptographically, but ensured via game theory.

### Self-Interested Clients as Verification Layer

Traditional approach: An indifferent chain continuously verifies all storage, regardless of whether anyone cares.

Our approach: Interested clients verify their own storage, as a byproduct of using it.

The math is compelling. Suppose a client spot-checks 3 random chunks weekly. If a provider deletes 10% of their data:
- Probability of missing deletion per week: 0.9³ = 72.9%
- After 3 months (13 weeks): 98% detection probability
- After 6 months (26 weeks): 99.97% detection probability

And that's just explicit very low-rate spot-checking. Every normal read is also
implicit verification. A backup app that restores files is verifying storage. A
website visitor loading an image is verifying storage. The "verifier's dilemma"
(verification is too expensive) disappears when verification is free bandwidth.

### Automation Removes the Human Factor

The verifier dilemma does not hold, if checking is effortless: Which is the case
for storage. The assumption is that the very client software you use for
accessing the storage, making deals, extending deals, will silently in the back
fetch random chunks from providers and will challenge them on chain if chunks
are not provided. This is effortless, because bandwidth is cheap/free for
clients nowadays most of the time (flat-rate) and because it is automated: There
is no human effort involved.

The client software—the backup app, the file browser, the media player—performs verification automatically. When you
open your backup app, it spot-checks a few random chunks in the background. When you browse a folder, the client fetches
the directory listing chunks—verifying they exist and match their hashes. When you play a video, every chunk delivered
is verified against its hash.

This happens without user action, without user awareness, without user discipline. The lazy human doesn't need to
remember to verify. The software does it continuously, invisibly, as part of normal operation. Defaults matter.

### Objective Reliability Emerges from Subjective Checks

All this subjective verification aggregates into objective reliability. There are two trust questions:

**Trusting a provider (for your own bucket)**: Providers have on-chain track records—agreements completed, extensions,
burns, challenges received (counted per challenger tier) and failed. A provider with 100 successful agreements, 80% extension rate, and zero failed
challenges is probably reliable—not because they claim to be, but because 100 paying clients verified them over time.
(See [Client Strategies](#client-strategies) for practical selection criteria.)

**Trusting a bucket (someone else's data)**: How do you trust that a bucket you don't control will remain available?
Look at who else cares:
- **Replica diversity**: How many independent providers are storing replicas? What are their stakes?
- **Stakeholder diversity**: Who funded these replicas? Major institutions? Community members?
- **Data importance**: If this bucket has replicas from providers across multiple jurisdictions with significant stake,
  many parties have skin in the game.

The more independent stakeholders with economic interest in a bucket's availability, the stronger the guarantee—even if
you never verify yourself.

### The Last Resort: Challenge It Yourself

What if you don't trust aggregate metrics? What if you have strict requirements?

**Add your own replica.** Anyone can create a replica agreement with any provider they choose. Pick a provider you
trust, pay them directly, verify them yourself. Now you have at least one replica whose reliability you've personally
established.

Or simply **challenge directly.** Anyone can challenge any provider for any data they have a commitment for (exception: a private bucket's primaries accept challenges from members only). Don't trust
that a provider still has the data? Fetch one random chunk. If they respond, you've verified (and recovered that chunk).
If they don't, you challenge, they get slashed, and the world learns they're unreliable.

Challenging always costs the on-chain price of the provider posting the chunk
and its proofs—it is never free, for anyone, and it is never cheap. What differs
is who covers it. If the data is your own (you're a bucket member or run a
replica), the provider is made to bear part of that cost, but you still pay at
least half. The split is *leverage* to pressure the provider into serving, not a
way to make recovery affordable. You can keep challenging chunk by chunk, but at
this price recovering anything sizeable is a last resort, not a bulk channel. If
it's someone else's bucket and you're an outsider, you cover the full cost
yourself. Either case is enough to detect a dead provider and extract a chunk
you truly need; neither is a cheap way to pull data at scale. If you need more,
fund your own replica (full control), or—in the read-incentive phase—pay the
provider directly for bandwidth. If that turns out necessary, we might also bring
back the cost split in a capped form for the general public.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                           ON-CHAIN                                  │
│                                                                     │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │                         BUCKET                                │  │
│  │  ├── members: [Admin, Writers, Readers]                       │  │
│  │  ├── min_providers: 2                                         │  │
│  │  ├── snapshot: { mmr_root, start_seq, leaf_count }            │  │
│  │  ├── primary_providers: [A, B]  (admin-controlled)            │  │
│  │  └── storage agreements:                                      │  │
│  │      ├── Provider A: { Primary, max_bytes, payment, ... }     │  │
│  │      ├── Provider B: { Primary, max_bytes, payment, ... }     │  │
│  │      ├── Provider C: { Replica, sync_balance, last_sync, ... }│  │
│  │      └── Provider D: { Replica, sync_balance, last_sync, ... }│  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
│    Chain touched for:                                               │
│    • Bucket creation and membership (once)                          │
│    • Storage agreement setup (per provider)                         │
│    • Checkpoints (infrequent, batched)                              │
│    • Replica sync confirmations (periodic)                          │
│    • Dispute resolution (rare, game-theoretic deterrent)            │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
                                    ▲
                                    │ rare
                                    │
┌─────────────────────────────────────────────────────────────────────┐
│                          OFF-CHAIN                                  │
│                                                                     │
│   ┌─────────────┐    writes     ┌─────────────┐                     │
│   │   Client    │ ────────────> │  Primary    │                     │
│   │             │               │  Provider   │                     │
│   └─────────────┘               └─────────────┘                     │
│          │                             │                            │
│          │ reads                       │ sync                       │
│          ▼                             ▼                            │
│   ┌─────────────┐               ┌─────────────┐                     │
│   │  Primary or │               │   Replica   │ (syncs from         │
│   │  Replica    │               │   Provider  │  primaries/replicas)│
│   └─────────────┘               └─────────────┘                     │
│          ▲                                                          │
│          │ discovery: bucket → agreements → provider endpoints      │
│          └──────────────────────────────────────────────────────────│
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
                                    │ read scalability
                                    ▼
┌─────────────────────────────────────────────────────────────────────┐
│                      PROOF-OF-DOT                                   │
│                                                                     │
│   Sybil resistance, identity, read priority                         │
│   See: Proof-of-DOT Infrastructure Strategy                         │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

### Buckets: Stable Identity in a Fluid Provider Market

The fundamental unit is the **bucket**—an on-chain container that groups related data.

```
Bucket (on-chain, stable identifier)
├── bucket_id: u64              // permanent, never reused
├── members: [Admin, Writers, Readers]
├── min_providers: u32          // quorum for checkpoints
├── primary_providers: [A, B]   // admin-controlled, max ~5
├── snapshot: { mmr_root, start_seq, leaf_count }
└── storage agreements → providers
```

**Why buckets, not just content hashes?**

A content hash names data but doesn't guarantee anyone stores it. A bucket makes availability explicit and controllable:

- **Stable identity**: The bucket_id never changes, even as providers come and go. Applications reference buckets, not
  providers. Switch providers without breaking links.

- **Explicit availability**: On-chain state shows exactly which providers have agreements. No guessing, no DHT lookups,
  no hoping.

- **Mutability with history**: Bucket contents evolve (new files, updates), but MMR commitments provide immutable
  snapshots at any point. Version N is always accessible even after version N+1 exists.

- **Permissionless persistence**: A frozen bucket (append-only) can be funded by anyone—not just the owner. You care
  about open-source documentation? Fund a replica. You care about historical records? Extend the agreements. Data
  survives even if the original owner disappears.

### Two Classes of Providers

Providers fall into two categories with different trust models:

**Primary Providers** (admin-controlled):
- Added only by bucket admin
- Receive writes directly from clients
- Count toward `min_providers` for checkpoints
- Limited to ~5 per bucket (prevents bloat)

**Replica Providers** (permissionless):
- Added by anyone (you, a third party, a charity)
- Sync data autonomously from primaries or other replicas
- Paid per successful sync confirmation
- Unlimited count

**Why this split?**

Writes need coordination—someone must order appends to the MMR. This is why writes are restricted to writer and admin accounts.

But reads don't need coordination. Any provider with the data can serve it. Replicas provide permissionless read
redundancy. Even if an admin is compromised or malicious, replicas ensure the data remains accessible from independent
sources.

This creates a spectrum:
- **Centralized**: Single admin, few primaries, no replicas
- **Federated**: Admin with primaries, community-funded replicas
- **Permissionless**: Frozen bucket, anyone can add replicas, admin has no special power

### Bucket Visibility & Access

> On a **private** bucket, primary providers serve reads only to **members**
> (Admin, Writers, Readers). On a **public** bucket they serve anyone.

That is the whole mechanism. Everything below is consequence, and the
consequences are subtle—so read carefully before relying on `private`.

**Members are the access list; the Reader role earns its keep here.** Admins and
Writers can already read, so on a public bucket a Reader member would be
redundant—anyone can read. The Reader role exists precisely for private buckets:
it grants read access *without* write access, to accounts the admin chooses.
Adding a Reader to a public bucket is harmless and—read-wise—inert until the
bucket is made private. Not *entirely* inert, though: membership of any role also
confers the authorized challenger tier (see
[The Challenge Game](#the-challenge-game)), on public buckets too.

**Privacy is a property of primaries only—replicas always serve everyone.** A
replica does not gate reads; the flag is not a replica's concern. The only thing
`private` changes for a replica is operational: it knows the primaries will
refuse it, so it syncs from other replicas—or any data holder—instead. This is not a leak to be
patched—it is deliberate, and it is the source of the anti-censorship guarantee
below. (It also means a replica funder never needs read access to the primaries:
they read their own replica, which is public.)

**`private` is not enforceable, and it is not confidentiality.** The chain cannot
observe off-chain serving, so a primary that ignores the flag and serves a
non-member faces no on-chain penalty. `private` is therefore a *cooperative
request* honored by honest primaries, not a guarantee the protocol enforces. The
real confidentiality guarantee is **client-side encryption** (providers see only
encrypted bytes; see [Data Model](#data-model)). `private` is defense-in-depth on
top of that: it keeps honest and casual parties from fetching even the
ciphertext, hides read access patterns from non-members, and lets an admin
restrict who an honest primary serves. It is access control, not a cryptographic
seal. Note also that on-chain metadata—the MMR root, sizes, leaf counts,
checkpoints, challenge events—is public regardless of the flag; `private` gates
only the off-chain serving of bytes. The bytes themselves can still surface
on-chain through challenges: a valid response posts the challenged chunk
publicly, forever. On a private bucket only *members* may challenge primaries
(see [The Challenge Game](#the-challenge-game))—strangers cannot force private
bytes onto the chain—but any member doing so leaks that chunk by choice.
Encryption makes the leak worthless.

**It hardens freeloading only in one narrow case—don't rely on it.** A
freeloading provider (stores nothing, proxies from others at read or challenge
time) needs a live source. Provider accounts are not members, so on a private
bucket honest primaries refuse them like any other non-member—a freeloader's
only honest source is a replica (replicas serve everyone). Consequently:
whenever any replica exists, freeloaders of either kind have a source and the
flag does nothing. On a born-private bucket with no replicas, though, a
freeloading primary has no honest source at all and the first member challenge
catches it—effectively a permanent form of
[Isolation Mode](#isolation-mode). Still not a defense to *rely* on: it is
cooperative (a colluding provider ignores the flag and serves its partner) and
it evaporates the moment one replica appears. Freeloading is properly addressed
by latency measurement, stake, and isolation (see
[Freeloading Prevention](#freeloading-prevention)), not by visibility.

#### Replicas and visibility: no on-chain gate

A replica is only useful if it can obtain the data, which suggests a tempting
rule: allow a replica agreement only if a sync source exists (the bucket is
public, or a replica already exists). We deliberately do **not** enforce
this—the chain performs no syncability check at all, for three reasons:

1. **The chain cannot see the sources.** Everything is content-addressed:
   chunks self-verify against the committed MMR root, so a replica can obtain
   the data from *anyone* who holds it—the client itself, a member, an
   ex-replica whose agreement expired, a friendly stranger who fetched it while
   the bucket was public. Syncing from primaries or other replicas is the
   default transport, not the only channel; a replica can just as well accept a
   push from any data holder, verifying every byte trustlessly. On-chain
   agreements are a poor proxy for this set: a rule based on them rejects
   fulfillable agreements (data available off-chain) and admits doomed ones
   (the one listed replica may be dead, or expire tomorrow).

2. **A gate would help the censor.** Consider the worst case replicas exist
   for: a hostile admin, a private bucket, and the last replica agreement
   expiring. A sync-source rule would then forbid new replicas *forever*—the
   chain itself would finish the censorship job. Without a gate, any party
   still holding the bytes can seed a fresh replica at any time.

3. **Fulfillability is the funder's own business.** The client has the money at
   stake and is the last mover (it submits the provider-signed terms,
   `establish_replica_agreement`)—and only the client knows the sourcing plan,
   which need not be visible to anyone else: it can push the data to the
   replica itself, or arrange for a third party to. If the replica never
   obtains data, that is the funder's loss and no one else's; the chain has no
   reason to intervene, and neither could it—or the provider—judge
   fulfillability better than the client. Client software *should* surface
   this, though: when no sync source is visible on-chain (private bucket, no
   replicas), warn the funder and ask for confirmation that they have a
   sourcing plan.

So replica creation is never gated. For serving, the flag remains a purely
cooperative signal to honest primaries; its single on-chain effect lives in the
challenge rules instead (members-only challenges against a private bucket's
primaries—see [The Challenge Game](#the-challenge-game)). A born-private bucket
remains the strongest privacy configuration, not because anything forbids
replicas but because no honest source for one exists (only the admin-controlled
primaries ever held the data). And if the data does leak, no on-chain rule
could have stopped a replica anyway: real secrecy is encryption.

#### Transitions

Both transitions are always allowed (no preconditions), but they are not
symmetric in effect:

- **Public → private.** Primaries start gating going forward. This does **not**
  recall anything: any replica that already synced keeps serving the data it
  holds—to everyone—and can even seed further replicas from itself. New content
  appended after the switch does not reach non-members through honest primaries,
  and therefore cannot reach the replica set at all (no replica can cross the
  primary gate to fetch it—unless the admin itself has made a replica provider's
  account a member, which is the admin's own choice, not a hole). So with honest
  primaries the replica set is frozen at the content that existed when the switch
  happened: **old content stays public, only new appends become member-only.**

- **Private → public.** Discloses everything going forward and hands new
  replicas an easy sync source (the primaries). The flag itself can be flipped
  back, but the disclosure cannot be undone—data served or replicated while
  public stays out.

**Anti-censorship is the one guarantee that *is* enforced—and it cuts against
privacy.** Enforced by two on-chain rules, not by cooperative serving: replica
agreements cannot be early-terminated (the admin has no special rights over
them—they run to expiry), and a replica must answer challenges from *anyone*
(`challenge_replica`) or lose its entire stake. So an admin cannot silence
content a replica already holds by flipping the bucket private. That is intentional: replicas exist partly to protect the public against
a compromised or hostile admin (see "Why this split?" above). The honest
user-facing rule is therefore: **be careful what you make public.** Strong
privacy is only achievable by creating the bucket private and keeping it that
way—then a replica never gets a source through honest primaries (nothing
on-chain forbids one; it simply has nothing to sync from unless the data leaks).

### The Chain as Credible Threat

In normal operation, clients and providers interact directly:
- **Writes**: Client uploads chunks to provider, provider signs commitment
- **Reads**: Client fetches chunks from provider, verifies hashes
- **Storage**: Provider keeps data on disk, serves requests

The chain is touched only for:
- **Bucket creation**: Once
- **Agreement setup**: Per provider
- **Checkpoints**: Can be made infrequent, depending on use case
- **Sync confirmations**: Replicas confirm they've synced to a checkpoint - frequency depending on use case
- **Disputes**: Rare, expensive, avoided by rational actors

Most heavy load is expected from checkpoints and sync confirmations, both scale with the number of writes and both can
be batched to reduce load if necessary.

**Immediate guarantees without chain writes:**

When a client uploads data, the provider returns a signed commitment—a signature over the new MMR state including the
client's data. This signature is the client's guarantee:

1. **With signature only**: Client holds provider's signature. If provider denies having the data or refuses to serve,
   client can prove provider committed to storing it. The signature enables a challenge.

2. **After checkpoint**: The MMR root is on-chain, establishing the canonical bucket state. This adds:
   - **Synchronization**: All parties agree on the bucket's state at that point
   - **Public verifiability**: Anyone (for a private bucket's primaries: any member) can challenge based on the on-chain commitment, not just signature holders
   - **Multi-provider attestation**: Multiple primaries signed the same state
   - **Durability**: The commitment is in chain history—can't be lost if client loses the signature

Compare to Filecoin (including PDP):
- Data must be committed to a ProofSet **on-chain** before any guarantee exists
- Until the chain transaction confirms, you have only the provider's word
- Batching is possible, but guarantees still wait for chain confirmation

This is a key difference. We get immediate off-chain guarantees via signatures; checkpoints batch to the chain for
synchronization and public verifiability, but aren't required for the guarantee to be actionable.

**How signature-based guarantees work**

The provider's signature is cryptographic proof they acknowledged receiving the data. If the provider later refuses to
serve it:
- Client initiates challenge, presenting the signed commitment
- Provider must produce the data or lose their entire stake
- The signature proves the provider can't claim "I never had that data"

On-chain challenges are expensive: the provider must post the chunk and its Merkle proofs as a transaction—orders of
magnitude costlier than serving the same bytes off-chain. The challenger's deposit covers that on-chain cost up front,
so a challenge doesn't leave the provider out-of-pocket on its own. What deters the provider is what happens next, and it
depends on who is challenging.

When the challenger is one of the provider's own counterparties (the accounts it accepted agreements with, plus bucket
members), the provider is made to bear a share of that cost—so ignoring their off-chain requests until they resort to a
challenge costs it money. *That* is the deterrent: to avoid paying its share, the provider just serves them directly.
Strangers, by contrast, fund their challenges in full and cannot make the provider pay anything—so they impose no
monetary deterrent (only on-chain work and reputation damage). That asymmetry is deliberate: a provider's monetary
exposure is bounded to the counterparties it chose to deal with.

The expensive on-chain path exists to make the cheap off-chain path incentive-compatible.

---

## Economic Model

### Storage Agreements

Every provider-bucket relationship is governed by a storage agreement:

```
StorageAgreement
├── owner: AccountId        // can top up, transfer ownership
├── max_bytes: u64          // quota for this provider
├── payment_locked: Balance // prepaid storage payment
├── price_per_byte: Balance // locked at creation
├── expires_at: Block       // when agreement ends
├── role: Primary | Replica
└── (replica only) sync_balance, sync_price, last_sync
```

**Binding commitment**: Neither party can exit early. Provider committed to store for the agreed duration. Client
committed to pay for the agreed duration.

**Why binding?**
- Providers need predictability to provision storage
- Clients need assurance data won't be dropped mid-term
- Price volatility is handled by locking price at creation/extension
- Third parties can rely on data staying available until agreement expiration (at least)

### Provider Stake

Providers register with a global stake that covers all their agreements:

```
Provider
├── stake: Balance          // total locked stake
├── committed_bytes: u64    // sum of max_bytes across agreements
├── stats: { agreements, extensions, burns,
│            challenges_received_authorized,  // responded-to, from counterparties
│            challenges_received_public,      // responded-to, from strangers
│            challenges_failed }              // slashed — tier-independent

```

**Full stake at risk**: A single failed challenge slashes the provider's *entire stake*, not just the stake for that
bucket. This makes cheating economics absurd—deleting 1% of data to save $0.12/year risks losing thousands of dollars in
stake.

### The Challenge Game

When a provider doesn't serve data, anyone can challenge on-chain (private
buckets restrict primary challenges to members—see below):

```
1. Challenger initiates
   - Specifies: bucket, provider, leaf_index, chunk_index
   - Deposits: generously over-estimated challenge cost (the tx cost of the
     provider's on-chain response, with margin for fee fluctuations)

2. Challenge window opens (~48 hours)
   - Provider must respond with chunk data + Merkle proofs
   - The provider pays the response transaction fee from its own account—never
     from its stake (normal tx fee mechanism)
   - Challenger can cancel anytime before the response (deposit returned, pays
     only the cancel tx fee)

3. Resolution
   - Valid proof: Challenge rejected. The provider's response fee is reimbursed
     from the challenger's deposit (in full, or only a fraction—see below).
     Any excess deposit is returned to the challenger.
   - Cancelled by challenger: Deposit returned (challenger paid only tx fees)
   - Invalid/no proof: Provider's full stake slashed; challenger made whole
     from the slash (deposit and tx fees refunded—no reward beyond costs)
```

**Stake is never touched on a valid response.** The only thing in play is who
pays the *transaction cost* of the on-chain response. The challenger's deposit
covers it; how much of that cost the provider is then made to bear depends on
who is challenging.

**Two tiers of challenger.** Every bucket has a set of **authorized accounts**:
its members (Admin, Writers, Readers) plus the owner of any storage agreement on
the bucket (so replica funders count too). Everyone else is the **general
public**. The tier is evaluated once, at challenge creation, and stored in the
challenge—becoming a member or agreement owner afterwards does not upgrade an
open challenge (nor does losing the status downgrade one).

**Visibility gates who may challenge primaries.** On a `Private` bucket,
challenges against *primary* providers are restricted to members (keyed on the
challenged provider's role, whichever challenge extrinsic is used): the general
public cannot read the data, so it has no legitimate reliance to verify—and an
unrestricted challenge would let any stranger force private bytes onto the
public chain (a challenge response is public, forever). Note this permission
check is membership-only, stricter than the cost-tier: owning a (permissionless)
replica agreement must not buy a stranger an extraction channel into the
primaries. Replicas remain challengeable by *anyone* regardless of visibility:
their content is public anyway (they serve everyone), and their public
challengeability is what carries the anti-censorship guarantee. This is the only
on-chain effect of the visibility flag.

- **Authorized accounts** get a *split*: the provider is made to bear part of the
  response cost, the challenger's deposit covers the rest. The challenger's share
  never drops below 50%, so even at its cheapest a challenge costs the challenger
  most of the (already high) on-chain price. This is not a cheap recovery channel
  even for the owner—recovering data at scale this way is unreasonably expensive.
  Its purpose is **leverage**: a counterparty can make ignoring its off-chain
  requests cost the provider something, pressuring it to just serve directly.
- **The general public** pays the response cost *in full*; the provider is
  reimbursed 100% and loses no money (it still does the on-chain work and takes
  the recorded-challenge reputation hit). The public can still detect and slash a
  dead provider, and recover a chunk if they truly need it—at full cost. Two
  reasons strangers get no split:
  1. A provider cannot realistically serve *everyone* equally well, so a
     stranger being made to wait is not evidence of fault the way a paying
     counterparty's unanswered request is.
  2. **Anti-DDoS.** If strangers got the split, a large crowd could each pay
     little (one challenge apiece) while collectively draining a provider. Making
     each stranger pay the full cost removes that attack—the cost to the
     attackers scales with the damage they inflict.

**Cost split for authorized challengers.** Illustrative numbers—the one firm
rule is that the **challenger's share never drops below 50%** (a lower floor
would make griefing cheap). Faster responses cost the provider less:

| Response time | Challenger pays | Provider bears |
| --- | --- | --- |
| Block 1 | 90% | 10% |
| Blocks 2-5 | 80% | 20% |
| Blocks 6-24 | 70% | 30% |
| Blocks 25-95 | 60% | 40% |
| Blocks 96+ | 50% | 50% |

The general public is not on this table: the challenger always pays 100%. (The
failure case—no or invalid response—is separate: the provider's full stake is
slashed and the challenger is made whole from it, per Resolution above.)

The net effect: a provider's *monetary* challenge exposure is bounded to the
counterparties it chose to accept—strangers can be a nuisance but can't drain
it—while a missing or invalid response always costs the full stake. Vetting whom
it signs agreements with is how a provider controls its risk.

### The Burn Option

At agreement end, the owner decides: pay the provider, or burn the locked payment.

**Why burn?**

Imagine a provider who technically kept the data but was slow, unresponsive, or hostile. Not slashable (data exists),
but not satisfactory. Burning is a punishment signal:
- Provider gets nothing
- On-chain record of burn damages reputation
- Future clients see: "This provider had X agreements burned"

In addition to the challenging mechanism, this burn instead of pay possibility is part of guaranteeing/incentivizing
good enough retrievability, not just storing.

Note: Burn can also mean to transfer the funds into the DAP (Dynamic Allocation Pool).

**Avoiding abuse**

Burning costs *more* than paying. When burning, the client loses the locked payment AND pays an additional premium
(governance-configurable, e.g., 10%) from their account. This premium is deducted at burn time—if the client lacks
sufficient funds, the burn fails and they must pay instead (or top up their account).

This design has several important properties:
- **Anti-griefing**: Spite burns cost the client extra, not just the provider
- **Anti-blackmail**: "Refund me or I burn" now costs the blackmailer—they can't credibly threaten without losing money
  themselves
- **Credible signal**: A burn means the client was so dissatisfied they paid extra to punish. This makes burns rare but
  meaningful.
- **Natural cooling off**: An angry client without liquid funds can't burn impulsively—they're forced to pay, which may
  be the right outcome anyway

### Freeloading Prevention

What stops a provider from storing nothing and fetching from other providers when challenged?

**Economics**: The freeloading provider risks their entire stake on other providers' reliability and cooperation. If the
other providers also freeload, everyone gets slashed. If the other providers refuse to serve (why help a competitor?),
the freeloader can't respond to challenges.

**Detection**: Freeloading adds latency. A provider fetching from elsewhere shows network delay; a provider reading from
local disk shows disk latency. Clients measuring random read latency can detect and avoid freeloaders.

**Isolation mode** (future): Admin temporarily blocks providers B and C from serving, then challenges A. If A can't
respond without fetching from B/C, A is caught.

### Collusion Resistance

What about multiple providers colluding to reduce physical redundancy or coordinate service degradation?

**Technical collusion (reducing storage):** Providers A, B, C coordinate—only A stores data, B and C proxy from A. This
fails because:
- Latency measurements detect proxying (see [Latency-Based Selection](#latency-based-selection-and-geographic-redundancy))
- Each provider still needs full stake at risk
- Savings minimal (~$20/month) vs. risk (thousands in stake)

**Organizational collusion (censorship):** Single entity runs providers globally, receives government pressure to
censor. Protection through economics:
- Censoring all replicas = all stakes slashed
- Pressure must exceed economic penalty to force compliance
- Permissionless replicas can't be controlled by original provider

We don't prevent collusion cryptographically. We make it economically irrational through stake requirements, practically
difficult through latency-based verification, and strategically unstable through client optionality and provider
competition.

In the end, the guarantees are provided by stake. Client and provider are getting aligned on risks. **If the content has
a real risk of getting censored somewhere, a colluding provider also faces a real risk of losing its stake.**

---

## Proof-of-DOT and Read Incentives

### Why Isn't Slashing Enough?

Storage agreements and slashing ensure data *exists*. If a provider deletes data and can't respond to a challenge, they
lose their entire stake. This creates strong incentives to keep data stored.

But slashing doesn't guarantee **quality of service**. A provider can technically fulfill their agreement while
providing terrible service:
- Serve data at 1kb/s (slow but not slashable)
- Randomly drop 50% of requests (frustrating but not slashable)
- Prioritize arbitrary clients over paying customers (unfair but not slashable)

**Example scenario:** Provider has 1000 simultaneous requests but only 100MB/s bandwidth:
- **Without payment incentives**: Serve all at 100KB/s each. Everyone slow, no slashing.
- **With payment incentives**: 100 paying clients get 800KB/s each, 900 free clients share remaining bandwidth.

Slashing ensures data can be retrieved. Payments ensure it's retrieved quickly and reliably.

### The Solution: Payment-Based Prioritization

Providers allocate scarce resources (bandwidth, IOPS, CPU) based on cumulative payment history. This creates natural
quality differentiation:

| Client type | Service quality |
| --- | --- |
| New client | Good (attract customers) |
| Occasional free user | Decent (avoid challenges) |
| Heavy free user | Degraded (incentivize payment) |
| Paying client | Best (retain revenue) |

Non-paying clients still get served (provider must avoid slashing), but paying clients get priority. This motivates
providers to upgrade infrastructure and provide good service. More replica nodes will join a bucket, if a profit can be
made by serving highly popular content.

### Identity Through Proof-of-DOT

To enable sustainable free tiers and prevent unbounded resource consumption, we need sybil-resistant identity.

**The problem with anonymous free tiers:**
- Attacker creates unlimited identities, exhausts resources
- IP-based rate limiting is unreliable — IPv6 prefix rotation, VPNs, and cloud providers make it easy to acquire many
  apparent identities cheaply
- Reputation tracking becomes unbounded (memory exhaustion)
- Honest free users get crowded out by sybil attacks

Proof-of-DOT (detailed in [issue #6173](https://github.com/paritytech/polkadot-sdk/issues/6173)) solves this:
- **Registration cost**: Lock DOT to create identity.
- **Parameters for storage**: Set for global scale (billions of expected participants), allowing normal growth while
  preventing attack-scale registration
- **Graceful degradation**: Serve anonymous users when capacity available, drop them first under load
- **Bounded  & meaningful reputation**: Can reliably track reputation for all registered peers

**Service tiers emerge naturally:**
1. **Anonymous** (no Proof-of-DOT): Served only when spare capacity exists, no reputation tracking
2. **Registered** (Proof-of-DOT): Always get basic service, reputation tracked, can build payment history
3. **Paying** (Proof-of-DOT + payments): Premium service based on payment history

**Distinction from Proof of Personhood:** Proof-of-DOT allows multiple identities per person (if they pay for each).
It's designed for abundant resources (bandwidth, connections) where we want sustainable economics and fast verification
(network level). Proof of Personhood is for truly scarce resources (votes, airdrops) where one-per-human matters
(blockchain level). They complement each other: proven people could get DOT for Proof-of-DOT registration for free.

### How Competition Drives Quality

The feedback loop is natural:
1. Clients experience service quality directly
2. Good service → continued payments
3. Poor service → switch providers or stop paying
4. Providers compete for paying clients

Example: A viral video receives 10,000 requests. Paying users stream instantly, free users buffer. Client software
suggests: "High demand content. Pay 1 cent for instant access?" - Or more realistically user sets a budget and the
client software optimizes like this automatically.

### Challenge as Price Ceiling

If a provider demands more than the challenge cost to serve data, clients can challenge on-chain instead. This caps
extortion attempts—rational providers price below the challenge threshold to avoid:

- Bearing a share of the response fee (the client here is a paying customer, hence an authorized challenger)
- The on-chain cost of posting the chunk and proofs (the data is served either way; the challenge only *adds* the chain cost). Here the client is a paying customer—an authorized challenger—so the provider bears a share of it.
- Getting no payment
- Reputation damage

Most competition drives prices well below this ceiling. It's a safety net against monopolistic behavior.

---

## Client Strategies

### Selecting Providers

Clients should evaluate providers on:

**Stake level**: Higher stake = more to lose = stronger incentive alignment. Match stake to data importance.

| Data importance | Example stake tier |
| --- | --- |
| Ephemeral (cache) | Any registered provider |
| Standard (backups) | Higher stake preferred |
| Critical (compliance) | Highest available stake |

*Note: Specific stake thresholds will emerge from market dynamics and can be refined based on production data.*

**Track record**: Check on-chain stats:
- Total agreements vs. agreements extended (extension = client satisfaction)
- Agreements burned (burn = client dissatisfaction)
- Challenges received (authorized vs. public) vs. failed (failed = catastrophic failure)
- Provider age (longer = more track record)

**Stake homogeneity**: Don't mix high-stake and low-stake providers for the same bucket. A 1000 DOT provider alongside a
10 DOT provider means the 10 DOT provider can safely freeload—they risk little while relying on the 1000 DOT provider.

### Latency-Based Selection and Geographic Redundancy

By tracking latency over time and shifting toward lower-latency providers, clients naturally sieve out freeloaders and
slow providers. This happens automatically as part of normal usage.

**Why this works**: Physics doesn't lie. Cross-region latency is unavoidable—EU to US adds ~60-80ms round-trip minimum.
A provider fetching from another region to serve you will always be slower than one serving from local storage. Over
time, latency tracking reveals:
- Freeloaders proxying from other providers
- Slow or overloaded providers
- Providers not actually in their claimed region

**Geographic redundancy emerges**: If a client sees consistently low latency from certain providers in a region and high
latency from others, the low-latency providers are genuinely serving from Europe. By selecting providers with
consistently good latency from different regions, you achieve verified geographic distribution—not by trusting claims,
but by measuring physics.

**Cross-region verification in practice**:
1. Select providers in distinct regions (EU, US-East, Asia)
2. Know expected latency per region from your location
3. Measure actual latency via random chunk reads
4. Compare within regions—if one EU provider shows 100ms when others show 20ms, they're suspect
5. Over time, shift toward consistently fast providers per region

### Automated Verification

Client software should verify automatically, invisibly:

**On every normal use**:
- Directory browsing verifies directory chunks
- File opening verifies file chunks
- Media streaming verifies sequential chunks

**Background sampling**:
- Weekly: 3 random chunks from providers
- Flag latency anomalies or fetch failures
- Track per-provider reliability over time

**The result**: Verification becomes a byproduct of usage, not a conscious task. The lazy human problem is solved by
disciplined software.

### When to Challenge

Challenges are expensive and adversarial only use them if the provider does not serve data at all/not sufficiently.
Don't challenge for routine verification—that's what spot-checking is for. Challenge is the nuclear option when the
provider has broken the social contract.

---

## Data Model

### Content-Addressed Chunks

All data is broken into fixed-size chunks (e.g., 256KB), each identified by its hash:

```
Chunk
├── hash: H256 = blake2_256(data)
├── data: bytes (up to 256KB)
```

Internal nodes in Merkle trees are also chunks—their content is child hashes:

```
Internal Node
├── hash: H256 = blake2_256(child_hashes)
├── children: [H256, H256, ...]
```

**Why content-addressed?**
- Deduplication: Identical chunks stored once
- Verification: Hash mismatch = corruption detected
- Cacheability: Any node can serve verified chunks

### MMR Commitments

Each bucket tracks state via a Merkle Mountain Range (MMR):

```
BucketSnapshot
├── mmr_root: H256       // root of the MMR
├── start_seq: u64       // first leaf sequence number
├── leaf_count: u64      // number of leaves
├── checkpoint_block: Block
├── primary_signers: BitVec  // which primaries signed
```

**Canonical range**: `[start_seq, start_seq + leaf_count)`

**MMR leaves** contain:
```
MmrLeaf
├── data_root: H256      // Merkle root of chunk tree
├── data_size: u64       // logical size of this data
├── total_size: u64      // cumulative unique bytes in bucket
```

**Append**: Add new leaf with new data_root
**Delete**: Increase start_seq (old leaves no longer in range)
**Freeze**: Lock start_seq—bucket becomes append-only forever

### Client-Controlled Layout

The protocol provides what is essentially a disk: content-addressed chunks of fixed size. Clients control layout completely.

Any filesystem technique works:
- Reserved chunks for metadata/directories (e.g., first chunk = root directory)
- Files referenced by byte offset + length
- Inodes, extent trees, FAT—whatever the application needs
- Encryption of all content including directory structure

**Example layout:**

```
Chunk 0-2: [encrypted directory structure]
Chunk 3-10: [encrypted file: photo1.jpg]
Chunk 11-15: [encrypted file: document.pdf]
...
```

The client reserves the first chunks for directory structure. With large chunk sizes (e.g., 256KB), multiple directory
levels fit in a single chunk. The client fetches chunk 0, decrypts directory entries, learns where files live (by byte
offset + length), and fetches. The provider sees only "client requested chunks 0, 3-10"—no semantic meaning.

**Alternative: one file per leaf.** A chat channel might store each media file as its own MMR leaf—the leaf's
`data_root` is simply the Merkle root of that file's chunks. No filesystem structure needed; the chat protocol tracks
which leaf corresponds to which message.

**Privacy by design**: Providers see only encrypted bytes. They learn nothing about file structure, metadata, or
content. The application layer—entirely client-controlled—imposes meaning on the chunks.

---

## Use Cases

### Personal Backup

```
Setup:
├── Create bucket (single admin)
├── Add 2-3 diverse providers
├── Encrypt locally with master key

Operation:
├── Incremental backup: content-defined chunking
├── Deduplication: unchanged chunks already exist
├── Spot-check: 3 random chunks weekly (automated)

Recovery:
├── Fetch from any provider (all have full copy)
├── Verify via hash, decrypt locally
```

### Media in Chat

```
Setup:
├── Creator makes bucket
├── Add members as Writers
├── 1-2 providers (low redundancy OK for ephemeral)

Operation:
├── Member uploads image → gets data_root
├── Message contains: {bucket_id, data_root, leaf_index}
├── Recipient fetches directly from provider

Concurrent writes:
├── Content-addressed: parallel uploads don't conflict
├── Commits coordinated via chat message ordering
```

### Public Website

```
Setup:
├── Public bucket (anyone can read)
├── Geographically diverse providers
├── Frozen (append-only for version history)

Discovery:
├── DNS TXT record: bucket_id + current leaf_index
├── Client picks fastest provider for their region

Updates:
├── New content → new MMR leaf
├── Update DNS to new leaf_index
├── Old versions remain accessible via old leaf_index
```

### Business Compliance Archive

```
Setup:
├── min_providers = 3
├── Frozen bucket (immutable audit trail)
├── 5 providers (stake homogeneity, infrastructure diversity)

Verification:
├── Continuous background sampling
├── Challenge on any anomaly
├── On-chain checkpoints = timestamped proof

Compliance:
├── Frozen = deletions impossible
├── Checkpoints = provider acknowledgments recorded
├── Slashing = accountability for data loss
```

---

## Comparison with Existing Solutions

### vs. Filecoin

| Aspect | Filecoin (PoSt) | Filecoin (PDP) | This Design |
| --- | --- | --- | --- |
| Proof mechanism | zk-SNARK | SHA2 Merkle | Game-theoretic |
| Proof frequency | Every 24h/sector | Every 24h/data set | On dispute only |
| Chain load | O(sectors × time) | O(ProofSets × time) | O(disputes) |
| Write latency | Hours (sealing) | Immediate | Immediate |
| Write guarantee | After chain tx | After chain tx | Immediate (signature) |
| Hardware | GPU required | CPU sufficient | Commodity |
| Best for | Cold archival | Hot storage | Hot interactive |

**The scaling difference:**

```
Filecoin (PDP) at 1M data sets:
  Chain load = 1M proofs/day + 1M proving-period txs/day
  ≈ 174M gas per data set per day → ~174T gas/day
  Chain budget ~29-144T gas/day → saturation

This design at 1M buckets:
  Chain load = setup + checkpoints + disputes
  With rational actors: disputes → 0
  Chain load: minimal, bounded by setup activity and consensus needing writes
```

**Our position:** Filecoin proves storage *exists*. We guarantee data is *retrievable*.

Filecoin proofs don't deliver data—a provider can pass every proof while refusing
to serve you. Our challenges extract actual chunk data on-chain. Chain recovery is
limited by throughput and cost, but still valuable:

1. **Last-resort recovery**: Your most precious 1GB of baby photos from a 2TB backup
   is extractable chunk-by-chunk.
2. **Pressure to serve**: Answering a challenge means posting the data on-chain—far
   costlier than serving it off-chain—and counterparties make the provider bear a share
   of that cost. So providers serve directly to avoid challenges entirely.

**When Filecoin is better:** Third-party verifiable audit trails—proof that data
existed at specific times, without being the paying client.

### vs. IPFS

| Aspect | IPFS | This Design |
| --- | --- | --- |
| What it is | Content-addressing protocol | Storage system |
| Discovery | DHT (best effort, records expire) | Chain (single query) |
| Persistence | No guarantees | Contractual + slashing |
| Read incentives | None | Proof-of-DOT priority |
| Mutable references | No (hash = content) | Yes (bucket = container) |
| Storage visibility | Hidden (who has this CID?) | Explicit (on-chain agreements) |

**Trade-off**: IPFS provides content addressing—useful as a naming/transfer layer. We provide storage guarantees on top.
They're complementary: buckets could use IPFS for chunk transfer while providing the accountability layer IPFS lacks.

### vs. Arweave

| Aspect | Arweave | This Design |
| --- | --- | --- |
| Model | Permanent, endowment | Contractual, renewable |
| Payment | One-time upfront | Ongoing agreements |
| Guarantees | "Forever" | Until agreement expires |
| Flexibility | Write-once | Mutable (unless frozen) |

**Trade-off**: Arweave optimizes for permanence with upfront payment. We optimize for flexibility with ongoing relationships.

---

## Rollout

### Phase 1: Buckets and Basic Storage

Deploy bucket infrastructure and storage agreements. Ecosystem providers offer initial storage (free or low-cost).

**Establishes:**
- On-chain discovery (bucket → agreements → providers)
- Working protocol implementation
- Initial application developers

### Phase 2: Challenges and Guarantees

Add challenge mechanism. Providers must stake. Clients can challenge and slash.

**Establishes:**
- Economic guarantees beyond reputation
- Slashing for misbehavior
- Trust model for critical data

### Phase 3: Proof-of-DOT

Add DOT staking for sybil resistance and read priority. Payment history tracking enables quality differentiation.

**Establishes:**
- Identity layer
- Quality-of-service tiers
- Foundation for provider competition

### Phase 4: Third-Party Providers and Replicas

Open to third-party providers. Add permissionless replica agreements.

**Establishes:**
- True decentralization (permissionless participation)
- Provider competition on price and quality
- Redundancy beyond admin-controlled primaries

**Why this works:** Each phase is functional standalone. No bootstrap paradox (need users for providers, need providers
for users). The system works at every stage—it just gets better.

---

## Future Directions

### Protocol-Based Verification (Optional Premium)

For use cases requiring stronger guarantees than game-theoretic verification—particularly
replicas (which lack natural client verification) and fire-and-forget archival—optional
periodic proofs similar to Filecoin's PDP could be added as a premium feature. This can
be layered on later without changing the core protocol.

### Capped Split for the General Public

Today the general public gets no cost split (challengers pay the response in
full) to close the DDoS hole. A capped version could give the public *some*
leverage without reopening that hole: apply the split to anonymous challenges
too, but only up to a per-provider budget over a rolling window of X blocks.
Once the budget is spent, further public challenges revert to full pay until the
window resets. The budget caps the total a crowd can extract per window, so the
DDoS attack is bounded rather than open-ended. Left out of the initial design;
addable later without changing the core mechanism.

### Isolation Mode

Admins can instruct providers to temporarily refuse serving non-members, then challenge a specific provider. If that
provider was freeloading (fetching from others), they can't respond. Detects freeloading without on-chain enforcement.
Note: This was explained in more detail in the previous version of this doc, but became harder with the introduction of
replicas—which should not be controllable by the admin. Incentives still align as honest providers have an interest in
helping catch free-loaders. Latency measurements and high stake should get us very far though.

---

## Addressing Common Concerns

### Challenge Economics: Coordinated Griefing

**Concern**: Coordinated outsiders could grief a provider by flooding it with challenges.

**Response**:

This is exactly why the general public gets no cost split—it closes the flooding hole:

1. **Outsiders pay the full cost, provider pays no money**: A public (non-authorized) challenger funds 100% of the provider's response cost; the provider's response fee is fully reimbursed and its stake is untouched. A crowd flooding a provider each pays a full deposit plus a tx fee per challenge, while the provider loses no money—the attackers' cost scales with the damage, so there is no cheap collective drain. What a stranger *can* still impose is non-financial: on-chain response work in-window and recorded challenges (reputation noise). That nuisance can't grind the provider's stake down or extract value, which is what financial griefing would require.

2. **Only counterparties get the split**: A provider is made to bear a fraction of the cost only for its own members or agreement owners—accounts it *chose* to deal with (it accepted their agreement) or that the admin added.

3. **Challenge cancellation**: Any challenger can cancel before the response, paying only the tx fee. If the provider serves off-chain after a challenge is initiated, the challenger cancels and the provider never even responds on-chain. Cancelled challenges leave no trace in the provider's stats (see next point).

4. **Reputation**: Challenge stats count only *responded-to* challenges (at resolution, never creation) and are split by tier—`challenges_received_authorized` vs `challenges_received_public`—so clients can weigh the two as they see fit. The challenges-*failed* count (the one that actually signals data loss) is unaffected, since the provider defends every one.

5. **Response capacity, not money, is the binding constraint**: the guarantees above hold only while the provider actually responds to every challenge within the ~48-hour window—a missed deadline costs the full stake regardless of the challenger's tier. Flood scale is bounded on the attack side (challenge creation is weight-limited per block, and each challenge locks a deposit covering a full response), but a provider must still provision its response pipeline—fee liquidity and transaction submission—for worst-case bursts.

### Collusion: Providers Sharing Storage

**Concern**: Providers could collude to reduce physical redundancy (all proxy from a single source).

**Response**:

Multiple detection mechanisms:

1. **Latency measurements**: A provider fetching from another location shows network latency. Physics doesn't lie — cross-region adds 60-80ms minimum. Clients measure per-provider latency over time and shift to consistently fast providers.

2. **Stake requirements**: Each colluding provider still needs full stake at risk. Savings from sharing storage (~$20/month) vs risk (thousands in stake) makes collusion irrational.

3. **Isolation mode** (future, see above): Admin temporarily blocks other providers, then challenges. If a provider was fetching from others, they can't respond.

4. **Natural selection**: Poor performance → clients don't renew agreements → provider loses revenue.

The design doesn't prevent collusion cryptographically. It makes collusion detectable and economically irrational.

### Passive Data: Files Never Read

**Concern**: Files never read receive no verification despite payment.

**Response**:

This is a design decision, not an oversight. The alternative is continuous proofs regardless of whether anyone cares — that's Filecoin's approach, and it doesn't scale.

For passive data that matters:

1. **Automated spot-checking**: Client software samples random chunks in the background. The user doesn't need to read files for verification to happen.

2. **Stake deterrent**: 3 random checks per week gives 98% detection over 3 months. Provider risks entire stake for minimal savings.

3. **Third-party verification**: Anyone caring about data can verify. Doesn't require being the owner.

4. **Optional periodic proofs** (future): For fire-and-forget archival requiring stronger guarantees, optional PDP-style proofs can be added as a premium feature. Layered on later without changing the core protocol.

The design optimizes for the common case: data someone actively cares about. For edge cases, mechanisms exist.

### Burn Option: Blackmail Channel

**Concern**: The burn option creates a blackmail channel ("refund me or I burn").

**Response**:

**Anti-blackmail mechanism**: Burning costs the client extra. When burning, client loses the locked payment AND pays an additional premium (e.g., 10%) from their own account. If they have insufficient funds, the burn fails.

Properties:

- Spite burns cost the blackmailer, not just the provider.
- "Refund me or I burn" now costs the blackmailer extra each time.
- A burn signals the client was so dissatisfied they paid extra to punish.
- Makes burns rare but meaningful.

Default behavior matters: most clients will pay (the default action). Burning requires an active decision and extra cost. This filters for legitimate dissatisfaction.

### Chain Bottleneck: Dispute Saturation

**Concern**: If disputes exceed chain capacity, the system breaks like Filecoin.

**Response**:

Fundamentally different scaling dynamics:

- **Filecoin**: chain load = f(storage committed). More storage = more proofs, regardless of actor behavior.
- **This design**: chain load = f(disputes). Disputes scale with misbehavior, not storage volume.

Why disputes → 0 in normal operation:

1. **Economic deterrent**: Provider risks entire stake for minimal savings. Rational actors don't cheat.
2. **Challenge cost**: Answering a challenge means posting the data on-chain—far costlier than serving it off-chain—and counterparties make the provider bear a share. They serve directly to avoid challenges.
3. **Reputation damage**: Challenges signal problems. Providers optimize to avoid them.
4. **Natural selection**: Bad providers lose clients and leave the market.

If disputes actually saturate the chain, something is fundamentally broken (massive irrational behavior or stake levels too low). Fix: governance increases stake requirements. Chain capacity is a ceiling for misbehavior, not active load.

---

## Summary

We've designed a storage system for the common case: data that someone cares about.

**The key insight:** Storage depends on payments, not proofs. When someone pays for storage, they care. When they care,
they verify—automatically, invisibly, as a byproduct of use. Cryptographic proofs for active data are redundant
overhead; for dormant data, they provide weaker guarantees than they appear (data still vanishes when payments stop).

**The architecture:** Buckets make storage relationships explicit—who stores what, with what stake, until when. No
hiding behind content hashes that obscure dependencies. Providers lock stake and face slashing. The chain exists as a
credible threat, not the hot path. Normal operations happen off-chain; the chain is touched only for setup, checkpoints,
and rare disputes.

**The scaling model:**
- Filecoin: O(storage × time) chain load—every sector/ProofSet proven periodically
- This design: O(disputes) chain load—with rational actors, approaches zero

**The result:** Storage capacity bounded by provider infrastructure, not chain throughput. Writes are instant. Reads are
fast. Guarantees are economic, not cryptographic—and for data with interested clients, economic guarantees backed by
slashable stake are both sufficient and more honest about what's actually being guaranteed.

**When to use something else:** For true fire-and-forget archival where you want objective proof data existed even if no
one ever reads it, Filecoin's continuous proofs add value. For permanent storage with upfront payment, consider Arweave.
We're optimized for interactive storage where someone is paying attention.

---

## References

### Filecoin

1. **Sealing and Sector Sizes**: Filecoin uses 32GB and 64GB sectors. Sealing takes ~4.5-5.5 hours wall-clock per
   sector (PreCommit1 alone ~3 hours, CPU-bound; remaining phases GPU-assisted).
   - [Sealing Pipeline | Filecoin Docs](https://docs.filecoin.io/storage-providers/architecture/sealing-pipeline)

2. **WindowPoSt (24-hour proof cycle)**: Every sector is proven once per 24-hour proving period, divided into 48
   deadlines of 30 minutes each.
   - [PoSt | Filecoin Spec](https://spec.filecoin.io/algorithms/pos/post/)
   - [What's Window PoST? | Trapdoor Tech](https://trapdoortech.medium.com/filecoin-whats-window-post-7361bfbad755)

3. **Proof of Data Possession (PDP)**: Launched May 2025, enabling hot storage verification without sealing.
   - [Introducing PDP: Verifiable Hot Storage on Filecoin](https://filecoin.io/blog/posts/introducing-proof-of-data-possession-pdp-verifiable-hot-storage-on-filecoin/)
   - [PDP Overview | Filecoin Onchain Cloud](https://docs.filecoin.cloud/core-concepts/pdp-overview/)
   - [FIP Discussion #1009 - Proof of Data Possession](https://github.com/filecoin-project/FIPs/discussions/1009)

4. **PDP Technical Details**: 24-hour proving period (2880 epochs) with a 30-minute submission window at its end;
   5 sampled leaves per proof, SHA2 Merkle proofs, no GPU required. ProofSets are mutable—can add/delete/modify
   without aggregation bottlenecks—but each needs its own proof transaction per period.
   - [FilOzone PDP Repository](https://github.com/FilOzone/pdp)
   - [PDP Installation Guide](https://docs.filecoin.io/storage-providers/pdp/install-and-run-pdp)

5. **Chain Throughput Constraints**: Pre-HyperDrive (2021), storage onboarding used >100% of chain capacity, limiting
   growth to ~40 PiB/day; verifying WindowPoSt consumed ~13% of network bandwidth (FIP-0010).
   - [HyperDrive Upgrade](https://filecoin.io/blog/posts/filecoin-v13-hyperdrive-network-upgrade-unlocks-10-25x-increase-in-storage-onboarding/)
   - [FIP-0010: Off-chain WindowPoSt Verification](https://github.com/filecoin-project/FIPs/blob/master/FIPS/fip-0010.md)

### IPFS

1. **IPFS Retrieval and Availability**: End-to-end retrieval medians of 2.9-4.4 seconds (SIGCOMM 2022); ~50% of
   one-year-old files fail a first 60-second retrieval attempt (SIGMETRICS 2024).
   - [Design and Evaluation of IPFS: A Storage Layer for the Decentralized Web](https://arxiv.org/pdf/2208.05877)
   - [A Closer Look into IPFS (SIGMETRICS 2024)](https://dl.acm.org/doi/10.1145/3656015)
   - [IPFS KPIs | ProbeLab](https://www.probelab.io/ipfs/kpi/)

### Network Latency

1. **Transatlantic Latency**: Round-trip times between EU and US hubs typically range 60-80ms, with theoretical minimum
   ~55ms based on speed of light in fiber.
   - Physical constraint: ~5,500km distance, light travels at ~200,000 km/s in fiber

### Detection Probability

1. **Spot-check Math**: For as little as 3 random checks per week with 10% data deletion:
   - P(miss per week) = 0.9³ = 0.729
   - P(detect in 13 weeks) = 1 - 0.729¹³ ≈ 0.98
   - P(detect in 26 weeks) = 1 - 0.729²⁶ ≈ 0.9997
