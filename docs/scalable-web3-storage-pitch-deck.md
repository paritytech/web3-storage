# Scalable Web3 Storage

---

## Problem: Current Solutions Don't Scale or Don't Guarantee What Matters

**IPFS**
- Protocol for content addressing, not a storage system
- No persistence guarantees
- No serving incentives
- DHT lookups: 2-10 seconds, frequently fail

**Filecoin**
- Chain throughput bounds storage capacity
- Continuous proofs: O(storage × time) on-chain load
- Proves storage existed at proof intervals
- Zero guarantees for retrieval
- Data disappears when payments stop regardless of proofs

**The Gap**: Cryptographic proofs don't prevent deletion when the deal expires. Proving you had data yesterday doesn't mean you'll have it tomorrow.

---

## Our Approach

Storage depends on payments, not proofs. When someone pays, they care. We take advantage of that.

**Three principles**:

1. Make storage relationships explicit (buckets show who stores what, with what stake, until when)
2. Verify offchain, only disputes are on-chain (challenges extract actual data on-chain)
3. Align economics so cheating is irrational (full stake slash for any failure)

Result: Storage capacity bounded by provider infrastructure, not chain throughput.

---

## Technical: No Cache Necessary

Full storage commitment completely off-chain. Best possible write latency.

**How it works**:
```
Client uploads → Provider signs commitment → Instant guarantee
```

The signature proves the provider acknowledged receiving data. If they later refuse to serve, client can challenge. Provider must produce data or lose entire stake.

**Compare to Filecoin**:

| | Filecoin (PDP) | This Design |
|---|---|---|
| Guarantee exists | After chain tx confirms | Immediately (signed commitment) |
| Write latency | Wait for chain | Instant |
| Cache needed | Yes (until confirmed) | No |

Chain confirmation (checkpoint) adds synchronization and public verifiability, but isn't required for the guarantee to be actionable.

---

## Cost: Much Cheaper and Adjustable

Chain touched only for:
- Bucket creation (once)
- Storage agreements (per provider)
- Checkpoints (infrequent, batchable based on needs)
- Disputes (rare, rational actors avoid)

**No expensive sealing**:
- Filecoin PoRep: 1.5-3 hours GPU time per 32GB sector
- This design: Zero sealing, data immediately accessible

**No continuous proofs**:
- Filecoin WindowPoSt: Every 24h per sector
- Filecoin PDP: Every 30min per ProofSet
- This design: Only on dispute

**Adjustable by use case**: Checkpoint frequently for critical data, batch for cost-sensitive applications. You control the trade-off.

---

## Scaling: O(disputes) vs O(storage × time)

**Filecoin at scale**:
```
1M ProofSets × 48 proofs/day = 48M proof txs/day
Chain saturation
```

**This design at scale**:
```
1M buckets:
Chain load = setup + checkpoints + disputes
With rational actors: disputes → 0
```

Storage capacity = f(provider infrastructure), not f(chain throughput)

---

## Better Guarantees: Time and Commitment

**Filecoin**:
- ✓ Provider had data at proof intervals
- ✗ Provider will have data tomorrow
- ✗ Data survives if deal expires

**This design**:
- ✓ Binding agreement (neither party exits early)
- ✓ Full stake at risk (catastrophic penalty)
- ✓ Challenge proves availability on-demand
- ✓ Valid for contract duration

Difference: "Data was there yesterday" vs "Data will be there for contract duration"

---

## Better Guarantees: Retrieval is Incentivized

Filecoin: Strong guarantees for storage, zero for retrieval.

This design: Same economic guarantee level for retrieval as storage.

**Mechanisms**:

1. **Challenge cost sharing**: Provider pays part even when responding correctly (10-50% depending on response speed). Serving directly is always cheaper.

2. **Burn option**: Client can burn payment + premium to punish poor service. On-chain record damages reputation.

3. **Track record**: Agreements completed, extended, burned, challenges received/failed. All visible on-chain.

Economic alignment: Provider earns by serving well, loses by serving poorly.

---

## Better Guarantees: Enforced Retrieval

Limited but useful: Challenges extract actual chunk data on-chain.

```
Challenge initiated → Provider submits chunk data + Merkle proofs → Data on-chain
```

**Use cases**:
- Last-resort recovery of critical data (most precious baby photos)
- Proof of availability for disputes
- Forcing delivery when provider unresponsive

**Limitations** (by design):
- Expensive (tx fees, on-chain storage)
- Limited by chain throughput
- Not for bulk recovery

The point: Provider knows you can force recovery, so they serve directly to avoid costs.

---

## Economics: Rational Honesty

Provider deletes 10GB to save costs:
- Savings: ~$0.12/year (at $0.001/GB/month)
- Risk: Entire stake slashed (e.g., $10,000)
- Detection: Even 0.01% probability = -$1 expected value

Rational choice: Keep the data.

**Challenge cost split** (incentive to avoid challenges):

| Response Time | Challenger Pays | Provider Pays |
|---|---|---|
| Block 1 | 90% | 10% |
| Blocks 2-5 | 80% | 20% |
| Timeout | 0% (refunded + reward) | 100% (slashed) |

Provider always pays something. Serving directly is always cheaper.

---

## Proof-of-DOT: Sybil Resistance for Free Tier

**Problem**: Without sybil resistance, free tier is unsustainable.

Provider wants to offer free tier for adoption, but faces:
- Memory exhaustion (can't track reputation for unbounded identities)
- Rate limit evasion (create new identity when throttled)
- Resource abuse (unlimited identities consuming free quota)

**Proof-of-DOT solution**:
- Identity costs DOT to register (locked at creation)
- Exponential pricing: nth participant pays base_price × 2^(n/step)
- Bounded set providers can track in memory/storage
- Parameters set for global scale (billions of expected identities)

**What it enables**:
- Providers can track free tier usage and reputation without memory exhaustion
- DDoS protection (creating attack-scale identities is expensive)
- Free tier remains sustainable

---

## Architecture: Explicit Storage Relationships

Bucket addressing makes storage guarantees transparent:

```
bucket://42/<data_root>      // Immutable (content hash in path)
bucket://42/fs/path          // Mutable (resolves to current state)
```

On-chain state shows:
- Who stores it: Providers A, B, C
- Guarantees: 1000 DOT stake each
- Duration: Until block 1,000,000
- Track record: Agreements, extensions, burns, challenges

**vs CID addressing** (`bafybei...`):
- Who stores it? Unknown
- What guarantees? Unknown
- How long? Unknown

Bucket = explicit relationship, stable & decentralized identifier for mutable
content. CID = hope someone cares.

---

## Data Model

**Content-addressed chunks**:
- Fixed size (e.g., 256KB)
- Hash = blake2_256(data)
- Deduplicated automatically

**Client-controlled layout**:
- Protocol provides chunks, client controls filesystem
- Any structure: reserved chunks, inodes, FAT, etc.
- Full encryption (including directory structure)
- Provider sees only encrypted bytes

**MMR commitments**:
- Versioned history (old versions always accessible)
- Frozen buckets (append-only, no deletions)
- Efficient proof generation

---

## Use Case: Personal Backup

Setup:
- Create bucket with 2-3 geographically diverse providers
- Encrypt locally
- Set checkpoint frequency (cost vs guarantee trade-off)

Operation:
- Incremental backup with content-defined chunking
- Automatic deduplication
- Automated spot-checking (3 random chunks weekly)

Recovery:
- Fetch from any provider
- Hash verification built-in
- Decrypt locally

---

## Use Case: Compliance Archive

Setup:
- Frozen bucket (append-only, deletions impossible)
- min_providers = 3 (quorum requirement)
- High stakes, diverse infrastructure

Verification:
- Continuous background sampling
- Challenge on anomaly
- On-chain checkpoints = timestamped proof

Compliance:
- Frozen = immutable audit trail
- Checkpoints = provider acknowledgments recorded
- Slashing = accountability for data loss
- MMR = versioned history

---

## Comparison

| | IPFS | Filecoin (PoSt) | Filecoin (PDP) | This Design |
|---|---|---|---|---|
| Persistence | None | Contractual | Contractual | Contractual + binding |
| Retrieval guarantee | None | None | None | Economic + enforceable |
| Write latency | Immediate | Hours | Fast (still chain) | Immediate |
| Write guarantee | None | After chain | After chain | Instant (signature) |
| Chain load | N/A | O(sectors × time) | O(ProofSets × time) | O(disputes) |
| Scaling bound | N/A | Chain throughput | Chain throughput | Provider infrastructure |
| Sealing | None | 1.5-3h (GPU) | None | None |
| Mutability | No | No | No | Yes (native) |
| Discovery | DHT (slow) | DHT | DHT | Chain (instant) |
| Transparency | Hidden | Hidden | Hidden | Explicit (buckets) |

---

## Addressing Review Concerns: Challenge Economics

**Concern**: Coordinated users could grief providers with challenges, paying only 90% of costs.

**Response**:

Multiple protections:

1. **Challenge cancellation**: Challenger can cancel anytime before response, getting full deposit back minus tx fee. If provider serves data off-chain after challenge initiated, challenger cancels and pays only tx fee.

2. **Provider pays even when honest**: This is intentional. Incentive is to serve directly and avoid challenges entirely, not to optimize challenge response.

3. **Economic rationality**: Coordinated griefing requires funding many challenges. At 90% cost, attacking a provider with 100 challenges costs attackers 9000 DOT to cost provider 1000 DOT. Cheaper to just not use that provider.

4. **Reputation damage**: Provider with many challenges (even successful responses) signals problems. Clients migrate to better providers.

Already incorporated: Challenge cancellation mechanism in design.

---

## Addressing Review Concerns: Collusion

**Concern**: Providers could collude to reduce physical redundancy (all proxy from single source).

**Response**:

Multiple detection mechanisms:

1. **Latency measurements**: Provider fetching from another location shows network latency. Physics doesn't lie—cross-region adds 60-80ms minimum. Clients measure per-provider latency over time, shift to consistently fast providers.

2. **Stake requirements**: Each colluding provider still needs full stake at risk. Savings from sharing storage (~$20/month) vs risk (thousands in stake) makes collusion irrational.

3. **Isolation mode** (future): Admin temporarily blocks other providers, then challenges. If provider was fetching from others, they can't respond.

4. **Natural selection**: Poor performance → clients don't renew agreements → provider loses revenue.

The design doesn't prevent collusion cryptographically. It makes collusion detectable and economically irrational.

---

## Addressing Review Concerns: Passive Data

**Concern**: Files never read receive no verification despite payment.

**Response**:

This is a feature, not a bug. The alternative is continuous proofs regardless of whether anyone cares—that's Filecoin's approach, and it doesn't scale.

**For passive data that matters**:

1. **Automated spot-checking**: Client software samples random chunks in background. User doesn't need to read files for verification to happen.

2. **Stake deterrent**: 3 random checks per week gives 98% detection over 3 months. Provider risks entire stake for minimal savings.

3. **Third-party verification**: Anyone caring about data can verify. Doesn't require being the owner.

4. **Optional periodic proofs** (future): For fire-and-forget archival requiring stronger guarantees, optional PDP-style proofs can be added as premium feature. Layered on later without changing core protocol.

The design optimizes for the common case: data someone actively cares about. For edge cases, mechanisms exist.

---

## Addressing Review Concerns: Burn Option

**Concern**: Creates blackmail channel (threaten to burn to extract refunds).

**Response**:

Already addressed in design:

**Anti-blackmail mechanism**: Burning costs client extra. When burning, client loses locked payment AND pays additional premium (e.g., 10%) from their account. If insufficient funds, burn fails and they must pay.

Properties:
- Spite burns cost the blackmailer, not just provider
- "Refund me or I burn" now costs the blackmailer extra
- A burn signals client was so dissatisfied they paid extra to punish
- Makes burns rare but meaningful

**Default behavior matters**: Most clients will pay (default action). Burning requires active decision and extra cost. This filters for legitimate dissatisfaction.

---

## Addressing Review Concerns: Chain Bottleneck

**Concern**: If disputes exceed chain capacity, system breaks like Filecoin.

**Response**:

Fundamentally different scaling dynamics:

**Filecoin**: Chain load = f(storage committed). More storage = more proofs, regardless of actor behavior.

**This design**: Chain load = f(disputes). Disputes scale with misbehavior, not storage volume.

**Why disputes → 0**:

1. **Economic deterrent**: Provider risks entire stake for minimal savings. Rational actors don't cheat.

2. **Challenge cost sharing**: Being challenged costs provider money even when responding correctly. They serve directly to avoid challenges.

3. **Reputation damage**: Challenges signal problems. Providers optimize to avoid them.

4. **Natural selection**: Bad providers lose clients, leave market.

If disputes actually saturate the chain, something is fundamentally broken (massive irrational behavior or stake levels too low). Fix: Governance increases stake requirements.

Normal operation: disputes rare. Chain capacity is ceiling for misbehavior, not active load.

---

## Two Provider Classes

**Primary Providers** (admin-controlled):
- Receive writes directly
- Max ~5 per bucket (prevents bloat)
- Count toward min_providers quorum
- Admin chooses trusted providers

**Replica Providers** (permissionless):
- Anyone can add replica for any bucket
- Sync autonomously from primaries/replicas
- Paid per successful sync confirmation
- Unlimited count

**Why this split**:

Writes need coordination (someone must order appends). Reads don't. Replicas provide permissionless read redundancy.

Even if admin is compromised, replicas ensure data remains accessible from independent sources.

---

## Rollout: Phased, No Bootstrap Paradox

**Phase 1**: Buckets and basic storage
- On-chain discovery works
- Ecosystem providers offer initial storage
- Applications can build

**Phase 2**: Challenges and guarantees
- Stake requirements
- Challenge mechanism
- Economic guarantees active

**Phase 3**: Proof-of-DOT
- Sybil resistance
- Quality tiers
- Sustainable free tier

**Phase 4**: Third-party providers
- Permissionless participation
- Provider competition
- Full decentralization

Each phase is functional standalone. System improves incrementally.

---

## What We Provide That Others Don't

1. Immediate off-chain guarantees (signed commitments)
2. Binding agreements (neither party exits early)
3. Retrieval incentivization (challenge costs + burn option)
4. Limited enforced retrieval (on-chain data extraction)
5. Transparent relationships (bucket addressing)
6. Native mutability (MMR with history)
7. Adjustable costs (checkpoint frequency)
8. O(disputes) scaling (unbounded by chain)
9. Full stake slashing (catastrophic penalty)
10. Proof-of-DOT integration (sustainable free tier)

No other system offers all of these.

---

## When to Use This vs Alternatives

**Use this design for**:
- Interactive applications
- Personal backups
- Content delivery
- Mutable data
- Cost-sensitive applications
- Need retrieval guarantees

**Use Filecoin for**:
- Third-party verifiable audit trails
- Cryptographic proof requirements
- Cold archival
- Can tolerate write latency

**Use Arweave for**:
- Permanent storage
- Upfront payment model
- Write-once data

Most Web3 storage use cases need availability and retrieval, not continuous cryptographic proofs.

---

## Summary

**Technical advantages**:
- No sealing → instant writes
- No continuous proofs → lower costs
- O(disputes) scaling → unbounded capacity
- Signed commitments → no cache needed
- MMR structure → native mutability

**Business advantages**:
- Adjustable costs → match budget to needs
- Binding agreements → predictable service
- Burn option → service quality accountability
- Transparent relationships → informed decisions

**Guarantee advantages**:
- Duration-based → "will be there" not "was there"
- Retrieval incentivized → actual availability
- Limited enforcement → last-resort recovery
- Full stake slashing → rational honesty

Storage that scales. Costs that adjust. Guarantees that matter.

---

## Documentation

Full design: `docs/scalable-web3-storage.md`
Implementation details: `docs/scalable-web3-storage-implementation.md`
