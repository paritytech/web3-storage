# Per-Bucket Store Design — One File, Two Table Groups

This document records the storage-layout decisions reached while evaluating
engines, because they turned out to matter more than the engine choice itself.
It is the design the [recommendations](02-recommendations.md) now assume:
**one database per bucket**, holding two groups of tables — a *content store*
and a *commitment store* — with durability chosen per transaction rather than
per file. It replaces both the current single-RocksDB layout and the earlier
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
| Tables | `chunks` (rowid) + `chunks_hash` index | `leaves`, `mmr_nodes`, `meta` |
| Key | content hash (32 B, uniformly random) | MMR position (dense integer; closed-form: `leaf_pos(k) = 2k − popcount(k)`) |
| Value | up to 256 KiB | 48 B leaves / 32 B nodes |
| Mutability | write-once, idempotent (re-store of same hash is a no-op) | write-once nodes; tiny mutable state row |
| Loss consequence | recoverable: hash-verified, re-fetchable from replicas or clients | **slashable** — this is what a signed commitment promises |
| Durability | transactions commit at `synchronous = NORMAL` (WAL append, no fsync) | transactions commit at `synchronous = FULL` — and, sharing the WAL, pin every content write before them |

**Why two table groups.** The two groups have opposite workloads (few huge
random-key values vs many tiny sequential-key values) and opposite durability
needs, and the atomicity boundary falls exactly between them: `mmr_root` is
*derived from* the MMR contents, so leaves + interior nodes + root must change
in one transaction — which is why the metadata row belongs **with** the MMR
tables rather than anywhere else. Keeping the groups distinct in the schema is
what lets each keep its own access pattern (payload-in-index for 48-byte rows,
a separate hash index for 256 KiB rows — see
[below](#what-the-benchmark-now-measures-for-this-design)).

**Why one file.** A database is one WAL, and WAL recovery is prefix-consistent:
whatever survives a crash is a prefix of the committed transactions, in order.
So when the commitment transaction commits at `synchronous = FULL`, the fsync it
pays durably pins **every content transaction that preceded it** in the same
file — the ordering invariant this design depends on becomes a property of the
engine rather than of application code. `PRAGMA synchronous` is per connection
and may be changed between transactions, so the two groups still pay different
durability prices; they just do it inside one file. The alternative — two files
per bucket — makes that invariant an application obligation and doubles the
per-bucket footprint; it is kept as the [documented fallback](#the-two-file-split-fallback)
and the reasons it was not chosen are given there.

**Why no global node pool.** The original design justified global
content-addressing with cross-bucket deduplication ("identical chunks stored
once"). Client-side encryption removes that benefit:
[the encryption design](../../drafts/CLIENT_SIDE_ENCRYPTION.md) generates a **fresh
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

- **One file means one open per challenge.** The high-frequency paths are
  single-store (checkpoint signing → commitment only; downloads → content only),
  but a challenge touches both groups, and with one file that is one LRU entry,
  one reopen (~87 µs on disk-backed storage, pass 4), 3 FDs. Under the two-file
  fallback the same challenge is two reopens and 6 FDs — irrelevant against a
  48-hour `ChallengeTimeout`, but it is why "×2" would not have been arguable
  away for challenge-heavy periods.
- **The cacheable half of a challenge stays cached.** The content-store reads
  are inherently cold — the provider cannot predict `chunk_index`; that
  unpredictability *is* the protocol — so no cache policy helps them. The
  commitment-store reads come from a ~10 MB working set that should stay hot.
  The concern with one file is that bulk chunk traffic evicts those pages from
  the shared pager cache. With `mmap_size` enabled — which the recommendation
  already requires for the content store's dedup and read latency — SQLite
  serves pages that live in the main file directly from the mapping without
  copying them into the pager cache, so chunk reads mostly bypass it; only pages
  still in the WAL go through the cache. The OS page cache was shared between
  two files anyway. This is reasoning, not a measurement — see the
  [fallback section](#the-two-file-split-fallback) for what would trigger a split.
- **The content store is not uniformly large-valued.** For a file of n chunks
  it holds n × 256 KiB chunks plus ~n × 64 B interior nodes, and the challenge
  path reads only tiny values plus one big one. The `content_store` benchmark
  scenario models the bytes (space, ingest, chunk serving) correctly, but not
  this mixed-size read pattern — a caveat if challenge latency is ever tuned
  for specifically.

## Crash consistency: what one WAL gives for free

The slashable state is a signed commitment that references missing data. The
write sequence that makes it unreachable:

```
1. content transactions: ingest chunks at synchronous = NORMAL   (WAL append, no fsync)
2. commitment transaction: append leaves + MMR nodes, update meta
   at synchronous = FULL                                          ← one fsync pins steps 1–2
3. sign the commitment                                            ← liability attaches only now
4. persist the Layer-1 index
```

Because both groups share one WAL, the fsync in step 2 durably records every
preceding content transaction too; WAL recovery replays a prefix, so a recovered
commitment implies the chunks it references are recovered as well. A crash
before step 2 loses only unreferenced chunks (client re-uploads; idempotent). A
crash after loses nothing that was signed for. No explicit
`wal_checkpoint(TRUNCATE)` barrier is required for correctness — that remains a
WAL-size housekeeping step, run on eviction.

What that removes, compared with a two-file layout: the choke-point requirement
(one function as the only path to the commitment commit), the crash-injection
test that guards it, and the startup scrub as a *safety* mechanism. What remains
worth doing anyway:

- **one shared write sequence** for the three upload paths (`api.rs`,
  `fs_api.rs`, `s3_api.rs`) — engineering hygiene now rather than a correctness
  boundary; the engine enforces the ordering whether or not the code is tidy;
- **a startup scrub** that walks committed leaves and verifies every referenced
  chunk is present — cheap and exact because content is content-addressed,
  and useful as defence-in-depth against bugs and media faults rather than
  against crashes. Signing happens after the commit, so anything it finds
  missing was never signed for and is repairable, not slashable;
- fsync failure is treated as fatal (recover from WAL), never retried;
- **the Layer-1 index is persisted last** — after the commitment commit. This
  requirement is unchanged and still hard, because `fs_indices/<bucket>.json`
  and `s3_indices/<bucket>.json` sit **outside** the database with no ordering
  relationship to it. The two failure modes are asymmetric: crashing before the
  index save orphans a committed leaf (wasted space), crashing the other way
  around leaves an index entry pointing at data no commitment covers
  (unservable, unprovable). Startup reconciliation drops dangling entries.

The commitment tables can still be over-protected cheaply. `PRAGMA
integrity_check(<table>)` limits the check to one table and its indexes, so
`leaves`, `mmr_nodes` and `meta` — ~10 MB per 100 k uploads — can be verified
after every checkpoint without touching the terabyte-scale `chunks` table; and a
snapshot of the slashable state is an `ATTACH` plus three `INSERT … SELECT`s,
still a ~10 MB copy, while the content is self-verifying by construction and
restorable via replica sync.

### The two-file split (fallback)

The alternative layout — `<bucket>.content.sqlite` and
`<bucket>.commitment.sqlite` — was the working design for most of this
evaluation, and it is sound. It is not the recommendation because its one
structural advantage over the single file turned out to be small when measured,
and its costs are structural:

- the ordering invariant above becomes application code: one choke point, a
  crash-injection test, a startup scrub as a safety net rather than a nicety;
- every hot bucket costs two connections, 6 FDs, two pager caches and two
  empty-file floors, and a challenge opens both;
- bucket deletion is six `unlink`s instead of three.

Three arguments were made for splitting. Weighed against the numbers already in
`results/`:

1. **`page_size` is per file, and the two groups want different values.** The
   content store wants 32 KiB (measured **1.58×** on chunk reads by shortening
   the overflow chain); the commitment store was expected to want 4 KiB because
   WAL journaling copies whole pages, so a small durable transaction dirtying
   ~log n pages would write 8× more WAL at 32 KiB. The
   [`page_size` sweep](01-storage-provider-benchmark.md#follow-up-is-sqlites-chunk-read-weakness-just-an-untuned-default)
   ran *every* scenario in one file at each page size, so it is exactly the
   single-file compromise, measured. At 32 KiB versus 4 KiB the commitment
   workload paid: durable 48 B appends **6,450 vs 6,956 op/s** (medians of three
   replicates; −7 %, inside the 4 KiB replicates' own 5,988–8,222 spread), proof
   read p50 **4.9 vs 3.9 µs**, reopen unchanged (73–86 vs 73–92 µs), 48 B disk
   amplification 1.28× vs 1.29×. The theoretical amplification does not surface
   in throughput. The one real price is the empty-bucket floor, 8 → 64 KiB
   (7.6 → 61 GiB per million *empty* buckets). Caveat: the sweep predates the
   pass-5 durability fix and mapped a `sync` batch to a full
   `wal_checkpoint(TRUNCATE)`, so the append figures are relative, not absolute.
2. **Pager-cache isolation.** Argued above to be largely moot once `mmap_size`
   is on, since mapped reads bypass the pager cache. Not measured: no scenario
   yet mixes chunk traffic and MMR reads in one file.
3. **Protection economics.** Addressed by per-table `integrity_check` and a
   table-level snapshot, as above.

**What would trigger the split:** a measured regression, on target hardware, in
commitment-side latency or checkpoint cost that is attributable to sharing the
file with chunk traffic — the mixed-workload scenario that the harness does not
yet have. Unlike sharded-vs-shared, **this decision is cheap to reverse**: it is
bucket-local, and moving a bucket between layouts is a table copy that can be
done lazily on next open. Different *engines* for the two groups (e.g. LMDB for
content, SQLite for commitments — see
[02](02-recommendations.md#why-not-lmdb-yet)) would also require it.

A further variant — a metadata-only database with chunks as loose
content-addressed files — remains the open question that
[report 01](01-storage-provider-benchmark.md#follow-up-is-sqlites-chunk-read-weakness-just-an-untuned-default)
raises; it is not evaluated here.

## What this design makes cheap

- **Append a leaf**: O(log n) — write the leaf + ~2 amortized interior nodes,
  not a blob rewrite + full rebuild.
- **MMR proof / peaks**: O(log n) point reads at *computable* positions — a
  prefetchable batch, no pointer-chasing. Directly bounds challenge-response
  latency, which spot-checking and slashing make the latency that matters.
- **Serve a chunk + proof**: O(log n) descent in the content store (or O(1)
  with a materialized per-file hash list — an optional index, decided
  separately).
- **Delete a bucket**: `unlink` the bucket's file with its `-wal` and `-shm`, so
  three paths; all bytes reclaimed synchronously.
- **Bulk ingest**: unsynced content transactions, one fsync at the commitment
  commit, instead of per-batch fsyncs.

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
The commitment tables keep payload-in-index: at 48 bytes per row that *is* the
sequential-insert packing that makes it cheap. Both shapes live in one file
without conflict — the choice is per table.

**Content store.** The `content_store` scenario models it exactly: 256 KiB
values under **random 32-byte keys**, unsynced batch ingest, a timed **flush
barrier**, then cold/warm random point reads — plus, since pass 5, the
absent-key lookup every upload performs before writing (`miss_latency`), which
is the dedup check and the one content-store read pattern the earlier passes
missed. The random keying matters: SQLite's space advantage under sequential
keys comes from a sequential-insert optimization (`balanceQuick`), and redb's
penalty from unconditional 50/50 page splits — both artifacts of sequential
keys, which the content store does not have. (The scenario's explicit flush
barrier is the two-file design's step; in the single file the same fsync is paid
by the commitment commit, so the figure stands in for that cost.)

**Commitment store.** `mmr_append_small` (durable 48 B batches, position keys)
and `proof_read` model it faithfully, and from pass 5 a durable batch means
`synchronous = FULL` — the setting the commitment transaction actually runs —
rather than a full WAL checkpoint per batch, which had been understating its
write throughput 17×.

**Per-bucket overhead.** Pass 4 added the costs that only appear when an engine
is instantiated once per bucket: OS threads, virtual address space, and the
`empty_floor` scenario (what a bucket holding nothing costs on disk, projected
to a million buckets). With one file per bucket they apply as measured — one
instance, one floor, 3 FDs per hot bucket. (They would be read ×2 under the
two-file fallback.)

**Page size.** The [`page_size` study](01-storage-provider-benchmark.md#follow-up-is-sqlites-chunk-read-weakness-just-an-untuned-default)
is the single-file configuration measured directly: one page size for the whole
file, 32 KiB buying 1.58× on chunk reads while costing the 48-byte commitment
rows a few percent on durable appends and nothing on space or reopen.

**Retired.** `node_append_large` and `disk_large` — 256 KiB values under
*sequential position keys with per-batch fsync* — modelled a layout this document
rejects, and were dropped in pass 5 for exactly the reason given above: the
content store is hash-keyed and durable at the commitment commit, and nothing in
the design writes large values under sequential keys.

Results and the per-store engine choice: [02-recommendations.md](02-recommendations.md).
