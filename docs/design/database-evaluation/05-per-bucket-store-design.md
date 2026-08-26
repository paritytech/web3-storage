# Per-Bucket Store Design — Content Store + Commitment Store

This document records the storage-layout decisions reached while evaluating
engines, because they turned out to matter more than the engine choice itself.
It is the design the [recommendations](02-recommendations.md) now assume:
**two databases per bucket** — a *content store* and a *commitment store* —
replacing both the current single-RocksDB layout and the earlier
"global node pool" idea.

## Where the current code actually stands

Two findings from reading `provider-node/src/storage/disk.rs` motivated
everything below; neither is visible in the benchmark reports alone.

1. **The entire MMR leaf vector is one serialized value.** `BucketState`
   (`disk.rs:30`) holds `leaves: Vec<MmrLeaf>` and is bincode-serialized into a
   single row of `CF_BUCKETS`. Appending one 48-byte leaf therefore reads,
   deserializes, re-serializes, and rewrites the whole blob — ~4.8 MB of I/O
   per append at 100 k leaves (~100,000× write amplification).
2. **MMR interior nodes are never persisted; the tree is rebuilt from scratch
   on every operation.** All four call sites (`commit`, `delete_before`,
   `get_mmr_proof`, `get_mmr_peaks`) run the same loop — `for l in
   &bucket.leaves { mmr.push(blake2_256(&l.encode())) }` — so every upload and
   every challenge response is O(n) in leaves (~2n blake2 hashes) where it
   should be O(log n). The challenge path is the one with an on-chain deadline.

Related read-path finding: `get_chunk_at_index` (`storage/mod.rs:212`) begins
with a **full DFS over the upload's entire chunk tree** to serve one chunk —
~8,192 reads for one 256 KiB chunk of a 1 GB file, versus 13 for a proper
root-to-leaf descent using the `children` links that are already stored. Chunk
serving (downloads, client spot-checks, challenges) should be O(log n) point
reads; no schema change is required to fix it, only descent instead of
collection.

## The two stores

| | Content store | Commitment store |
|---|---|---|
| Holds | chunks + chunk-tree interior nodes | MMR leaves, MMR interior nodes, bucket state (`mmr_root`, `start_seq`, `used_bytes`, quotas) |
| Key | content hash (32 B, uniformly random) | MMR position (dense integer; closed-form: `leaf_pos(k) = 2k − popcount(k)`) |
| Value | up to 256 KiB | 48 B leaves / 32 B nodes |
| Mutability | write-once, idempotent (re-store of same hash is a no-op) | write-once nodes; tiny mutable state row |
| Loss consequence | recoverable: hash-verified, re-fetchable from replicas or clients | **slashable** — this is what a signed commitment promises |
| Durability | relaxed per-write; **one flush barrier before commitment** | full sync on every transaction |

**Why the split.** The two groups have opposite workloads (few huge random-key
values vs many tiny sequential-key values), opposite durability needs, and the
atomicity boundary falls exactly between them: `mmr_root` is *derived from* the
MMR contents, so leaves + interior nodes + root must change in one transaction —
which is also why the metadata row belongs **in** the commitment store rather
than in a third database (a third file per bucket would also add ~50% to the
FD/RSS pool cost for the sake of five integers).

**Why no global node pool.** The original design justified global
content-addressing with cross-bucket deduplication ("identical chunks stored
once"). Client-side encryption removes that benefit:
[the encryption design](../CLIENT_SIDE_ENCRYPTION.md) generates a **fresh
random nonce per encryption**, so identical plaintext produces distinct
ciphertext even for the same client re-uploading the same file — dedup is
foreclosed by construction (AES-SIV-style determinism was considered and
rejected there). Dropping the global pool also:

- fixes a real accounting bug: today `store_node` charges `used_bytes` only on
  *global* novelty, so a bucket stores for free whatever any other bucket
  already stored, and quotas under-charge;
- makes bucket deletion a true `unlink` for the **bytes**, not just the
  metadata — the deletion win [report 01](01-storage-provider-benchmark.md)
  measures but the current layout cannot deliver (`delete_before` never removes
  nodes; chunk storage today grows monotonically);
- removes the need for cross-bucket reference counting / GC, which no document
  had specified.

Content-addressing is kept — verification and cacheability stand — only the
cross-bucket sharing goes.

## The challenge path reads both stores

A challenge (and every client spot-check — the same reads, issued voluntarily)
must produce three artifacts for `(bucket, leaf_index, chunk_index)`:

| Artifact | Store | Reads today (`challenge_responder.rs`) | Reads in this design |
|---|---|---|---|
| MMR proof for the leaf | commitment | O(n) — full leaf replay | O(log leaves) |
| Chunk-tree proof to `data_root` | **content** | O(n) — full-tree DFS | O(log chunks) |
| The chunk itself | **content** | 1 | 1 |

Two of the three artifacts come from the content store: the chunk-tree proof's
siblings are content-addressed **64-byte interior nodes that live there**, next
to the 256 KiB chunks. Three consequences:

- **Pool independence is statistical, not structural.** The high-frequency
  paths are single-store (checkpoint signing → commitment only; downloads →
  content only), so the two LRU pools can be sized independently — but a
  challenge opens both files. That worst case is two SQLite reopens (~87 µs each
  on disk-backed storage, pass 4) against a 48-hour `ChallengeTimeout`;
  irrelevant to latency, but it means "×2 FDs" cannot be argued away entirely
  for challenge-heavy periods.
- **Cache separation protects exactly the cacheable half of a challenge.** The
  content-store reads are inherently cold — the provider cannot predict
  `chunk_index`; that unpredictability *is* the protocol — so no cache policy
  helps them. The commitment-store reads come from a ~10 MB working set that
  stays permanently hot in its own pager cache, but would be evicted by bulk
  chunk traffic in a mixed file.
- **The content store is not uniformly large-valued.** For a file of n chunks
  it holds n × 256 KiB chunks plus ~n × 64 B interior nodes, and the challenge
  path reads only tiny values plus one big one. The `content_store` benchmark
  scenario models the bytes (space, ingest, chunk serving) correctly, but not
  this mixed-size read pattern — a caveat if challenge latency is ever tuned
  for specifically.

## The crash-consistency invariant (the one cost of two databases)

A single database gives an ordering guarantee for free: all column families
share one WAL, and WAL recovery is prefix-consistent, so a recovered MMR commit
implies the chunk writes that preceded it also survived. Two databases are two
WALs with no relationship, so the invariant becomes application code:

```
1. content_store: ingest chunks (unsynced batches — fast)
2. content_store.flush()          ← barrier: everything referenced is durable
3. commitment_store: append leaves + MMR nodes, update root (durable txn)
4. sign the commitment            ← liability attaches only now
```

The order is not negotiable: content durable *before* the commitment that
references it. A crash before the barrier loses only unreferenced chunks
(client re-uploads; idempotent). A crash after loses nothing that was signed
for. The **slashable state** — a signed commitment referencing missing data —
is unreachable if the sequence holds.

Discipline requirements, stated plainly:

- the sequence lives in **one** choke-point function; no other path may reach
  `commitment_store.commit()` (today three upload paths exist — `api.rs`,
  `fs_api.rs`, `s3_api.rs` — they must share it);
- a **crash-injection test** kills the process at every step and asserts on
  restart that no committed leaf references a missing chunk;
- fsync failure is treated as fatal (recover from WAL), never retried;
- a **startup scrub** walks committed leaves and verifies every referenced chunk
  is present. This is cheap and exact because content is content-addressed —
  verification is intrinsic, not a checksum bolted on. Its guarantee is the
  useful part: signing happens *after* the commitment commit, so anything the
  scrub finds missing was never signed for, which makes it repairable (re-fetch
  from a replica, or let the client re-upload) rather than slashable. It is the
  net under the invariant, not a substitute for it;
- the **Layer-1 index is persisted last** — after the commitment commit.
  `fs_indices/<bucket>.json` and `s3_indices/<bucket>.json` sit outside both
  databases with no ordering relationship to either, and the two failure modes
  are asymmetric: crashing before the index save orphans a committed leaf
  (wasted space), crashing the other way around leaves an index entry pointing
  at data no commitment covers (unservable, unprovable). Startup reconciliation
  drops dangling entries.

In exchange, the store whose loss slashes is ~10 MB per 100 k uploads and can
be over-protected for free (`synchronous=FULL`, `integrity_check`, even a
mirrored copy), while the terabyte-scale store is self-verifying by
construction and restorable via replica sync.

### The single-file fallback

If the barrier discipline above proves unenforceable — three upload paths, and a
convention no engine checks — **one SQLite file with two tables is a sound
design, not a broken one**, and it is worth being explicit about what it costs
and what it returns.

It returns the ordering guarantee. `PRAGMA synchronous` is per-connection and can
be toggled per transaction: run content transactions at `NORMAL` (no fsync, WAL
append only) and the commitment transaction at `FULL`. Because there is now one
WAL and WAL recovery is prefix-consistent, that single fsync durably pins **every
preceding content write**. The barrier stops being application code and becomes a
property of the engine — no choke-point requirement, no crash-injection
obligation, and half the file descriptors per hot bucket.

So the relaxed-durability argument, which is the intuitive reason to split, is
**the weakest** of the three. The split's load-bearing reasons are the other two,
plus one measured conflict:

1. **`page_size` is per file, and the two stores want opposite values.** The
   content store wants 32 KiB (measured **1.58×** on chunk reads by shortening
   the overflow chain). The commitment store wants 4 KiB, because WAL journaling
   copies **whole pages**: a checkpoint dirtying ~log n pages costs ~64 KiB of
   WAL at 4 KiB pages and ~1 MB at 64 KiB — a ~16× write amplification on the
   fully-synced store, paid on every checkpoint. One file forces one compromise
   that is wrong for one side.
2. **Pager-cache isolation.** Each connection has one page cache. Mixed, a burst
   of 256 KiB chunk reads evicts the MMR pages the challenge path needs hot.
   Split, the commitment store's ~10 MB working set stays resident permanently.
3. **Protection economics.** `integrity_check` on a 10 MB file is milliseconds
   and can run after every checkpoint; on a multi-GB mixed file it cannot.
   Snapshotting the slashable state after each signed commitment is a 10 MB copy
   rather than a copy of chunks that replicas already hold.

Unlike sharded-vs-shared, **this decision is cheap to reverse**: it is
bucket-local, and moving a bucket between layouts is a table copy that can be
done lazily on next open.

## What this design makes cheap

- **Append a leaf**: O(log n) — write the leaf + ~2 amortized interior nodes,
  not a blob rewrite + full rebuild.
- **MMR proof / peaks**: O(log n) point reads at *computable* positions — a
  prefetchable batch, no pointer-chasing. Directly bounds challenge-response
  latency, which spot-checking and slashing make the latency that matters.
- **Serve a chunk + proof**: O(log n) descent in the content store (or O(1)
  with a materialized per-file hash list — an optional index, decided
  separately).
- **Delete a bucket**: `unlink` the bucket's files — two databases, each with its
  `-wal` and `-shm`, so six paths; all bytes reclaimed synchronously.
- **Bulk ingest**: unsynced writes + one fsync at the barrier, instead of
  per-batch fsyncs.

## What the benchmark now measures for this design

**The content store's index must not hold its payload.** The dedup check —
`check_exists` before every chunk write — is on the upload hot path, and its cost
turns out to be decided by schema rather than engine. Storing chunks as
`hash BLOB PRIMARY KEY … WITHOUT ROWID` puts 256 KiB payloads inside the B-tree
that the hash lookup descends; giving the hash its own index over a rowid table
measures **50× faster on the dedup miss and 4.8× faster on chunk reads**, at
identical disk amplification. This is the single largest performance decision in
the whole evaluation, and it is not an engine choice — see the
[dedup experiment](01-storage-provider-benchmark.md#the-dedup-experiment-three-hypotheses-one-cause-one-fix).
The commitment store keeps payload-in-index: at 48 bytes per row that *is* the
sequential-insert packing that makes it cheap.

**Content store.** The `content_store` scenario models it exactly: 256 KiB
values under **random 32-byte keys**, unsynced batch ingest, a timed **flush
barrier**, then cold/warm random point reads — plus, since pass 5, the
absent-key lookup every upload performs before writing (`miss_latency`), which
is the dedup check and the one content-store read pattern the earlier passes
missed. The random keying matters: SQLite's space advantage under sequential
keys comes from a sequential-insert optimization (`balanceQuick`), and redb's
penalty from unconditional 50/50 page splits — both artifacts of sequential
keys, which the content store does not have.

**Commitment store.** `mmr_append_small` (durable 48 B batches, position keys)
and `proof_read` model it faithfully, and from pass 5 a durable batch means
`synchronous = FULL` — the setting this store actually runs — rather than a full
WAL checkpoint per batch, which had been understating its write throughput 17×.

**Per-bucket overhead.** Pass 4 added the costs that only appear when an engine
is instantiated once per bucket: OS threads, virtual address space, and the
`empty_floor` scenario (what a bucket holding nothing costs on disk, projected
to a million buckets). All of them, and `multi_instance`, should be read **×2**
under this design: two databases per bucket means twice the FDs, instances and
empty-bucket floor at a given pool size.

**Page size.** The [`page_size` study](01-storage-provider-benchmark.md#follow-up-is-sqlites-chunk-read-weakness-just-an-untuned-default)
belongs to this split: 32 KiB pages are worth 1.58× on chunk reads in the content
store and are worth nothing to 48-byte commitment rows, which is only a coherent
recommendation because the two stores are separate files.

**Retired.** `node_append_large` and `disk_large` — 256 KiB values under
*sequential position keys with per-batch fsync* — modelled a layout this document
rejects, and were dropped in pass 5 for exactly the reason given above: the
content store is hash-keyed and barrier-durable, and nothing in the design writes
large values under sequential keys.

Results and the per-store engine choice: [02-recommendations.md](02-recommendations.md).
