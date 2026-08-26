# Migration Plan

From today's **single shared RocksDB** to the recommended architecture:
**two SQLite stores per bucket** in the Storage Provider — a content store and a
commitment store, per
[05-per-bucket-store-design.md](05-per-bucket-store-design.md).
Sequenced to interleave with [Issue #100](https://github.com/paritytech/web3-storage/issues/100)
(per-bucket isolation), which this evaluation directly informs.

## Starting point

- **Storage Provider** — `provider-node/src/storage/disk.rs`: one `rocksdb::DB`
  with **four** column families (`disk.rs:20-24`):

  | CF | Holds | Destination |
  |----|-------|-------------|
  | `CF_NODES` | chunks + chunk-tree nodes, keyed by content hash, **global across buckets** | per-bucket content store |
  | `CF_BUCKETS` | `BucketState` — `mmr_root`, `start_seq`, `used_bytes`, `max_bytes`, and **all MMR leaves in one bincode value** | per-bucket commitment store |
  | `CF_METADATA` | the negotiation nonce high-water mark (`KEY_NONCE`, via `DiskNonceStore`) — **provider-global, not per-bucket** | provider-level store (below) |
  | `CF_ROOT_TO_BUCKET` | nothing — created at `disk.rs:68`, never read or written | **already dead**; drop it |

  Critically, `BucketState` serializes **all MMR leaves into a single value**, so
  every commit/proof/delete is O(n) on the whole leaf set — the core problem
  Issue #100 targets. The backend already sits behind the `StorageBackend` trait
  (`provider-node/src/storage/mod.rs`), selected at startup via `--storage-mode`.

The `StorageBackend` trait is the seam that makes the provider migration
low-risk — a new backend is a new trait impl, switchable by flag — **but it is
not a drop-in seam.** `get_node(&self, hash: &H256)` (`storage/mod.rs:153`) takes
**no `bucket_id`**: it is written against the global `CF_NODES` pool, and a
per-bucket content store cannot route a bare hash to a file. The trait's default
methods inherit the assumption (`collect_chunks`, `collect_chunk_hashes`,
`get_chunk_at_index`, `calculate_tree_size`), while `store_node` and
`check_exists` already take one. Every call site has a `bucket_id` in scope
(`fs_api.rs:164`, `s3_api.rs:155`, `api.rs:428`, `api.rs:563`,
`challenge_responder.rs:313`), so threading it through is mechanical — but plan
Phase 1 as **a trait change plus caller updates**, not a new impl alone. The
alternative — a resident hash→bucket map so the signature can stay — reintroduces
exactly the global state this design removes, and makes bucket deletion a map
mutation rather than an `unlink`.

---

## Storage Provider migration (couples with Issue #100)

The engine choice (SQLite) and the per-bucket isolation (Issue #100) are one
change: SQLite is *chosen because* of the per-bucket model, so implement them
together. The architecture decision itself is now evidence-backed — see the
sharded-vs-shared matrix in [report 01](01-storage-provider-benchmark.md).

### Phase 1 — `SqliteBucketStore` behind the existing trait

- Add a new `StorageBackend` implementation (e.g.
  `provider-node/src/storage/sqlite_bucket.rs`) that owns an **LRU pool of
  per-bucket SQLite connections** instead of one shared DB. **Two files per
  bucket**, because the two stores have opposite workloads and opposite
  durability needs:

  | File | Holds | Durability |
  |------|-------|-----------|
  | `{storage_path}/buckets/{bucket_id}.commitment.sqlite` | MMR leaves, MMR interior nodes, bucket state | `synchronous = FULL` — this is the slashable state |
  | `{storage_path}/buckets/{bucket_id}.content.sqlite` | chunks + chunk-tree interior nodes | `synchronous = NORMAL` + one flush barrier per commitment |

- **Commitment-store schema** (resolves the O(n) blob): store MMR leaves **per
  position**, not as one serialized vector, and persist the interior nodes that
  are today rebuilt on every operation —

  ```sql
  CREATE TABLE leaves (pos INTEGER PRIMARY KEY, leaf BLOB NOT NULL) WITHOUT ROWID;
  CREATE TABLE nodes  (pos INTEGER PRIMARY KEY, hash BLOB NOT NULL) WITHOUT ROWID;
  CREATE TABLE meta   (k TEXT PRIMARY KEY, v BLOB NOT NULL);  -- mmr_root, start_seq, used/max bytes
  ```

  `commit`, `delete_before`, and proof reads become **bounded range operations**
  keyed by position, instead of deserialize-mutate-reserialize of the whole set
  plus a full MMR rebuild. Leaves, interior nodes and `meta` live in one file
  precisely so a commit updates all three in **one transaction** — `mmr_root` is
  derived from the other two and must never disagree with them.

- **Content-store schema** — content-addressed, write-once. The hash index must
  be a **separate B-tree** from the payload, and `mmap_size` must be enabled:

  ```sql
  CREATE TABLE nodes (
    id       INTEGER PRIMARY KEY,
    hash     BLOB NOT NULL,
    data     BLOB NOT NULL,
    children BLOB
  );
  CREATE UNIQUE INDEX nodes_hash ON nodes (hash);
  ```

  Not a style preference: `hash BLOB PRIMARY KEY … WITHOUT ROWID` puts 256 KiB
  payloads inside the index that every dedup check descends, which measures **50×
  slower on the dedup lookup and 4.8× slower on chunk reads** — see the
  [dedup experiment](01-storage-provider-benchmark.md#the-dedup-experiment-three-hypotheses-one-cause-one-fix).
  The commitment store keeps `WITHOUT ROWID`, where 48-byte payloads make
  payload-in-index the right choice.

- **The write sequence is a choke point, not a convention.** Per
  [05](05-per-bucket-store-design.md#the-crash-consistency-invariant-the-one-cost-of-two-databases),
  two databases are two WALs with no ordering relationship, so one function must
  own: ingest chunks unsynced → `content.flush()` barrier → durable commitment
  transaction → sign → persist the Layer-1 index. The three existing upload paths
  (`api.rs`, `fs_api.rs`, `s3_api.rs`) must share it, and a crash-injection test
  must assert no committed leaf ever references a missing chunk.
- **The Layer-1 index is a third store with no ordering guarantee.**
  `fs_indices/<bucket>.json` and `s3_indices/<bucket>.json` are written by atomic
  rename, entirely outside both databases. Today's upload paths already save the
  index *after* `commit` (`fs_api.rs:105` then `:129`; `s3_api.rs:82` likewise) —
  keep that order, because the two failure modes are not symmetric: a crash
  between commit and index save leaves a committed leaf nothing references (an
  orphan — wasted space, no correctness or liability problem), whereas the
  reverse leaves an index entry pointing at data no commitment covers, which a
  reader can neither serve nor prove. Startup reconciliation drops dangling index
  entries.
- **Add a provider-level store for what is not per-bucket.** `CF_METADATA`'s
  nonce high-water mark belongs to the provider, not to any bucket, and the
  two-stores-per-bucket layout has nowhere to put it. Create
  `{storage_path}/provider.sqlite` (`meta(k TEXT PRIMARY KEY, v BLOB)`,
  `synchronous = FULL`) as the `NonceStore` backing and the home for future
  provider-global state. Without it the counter is silently dropped and
  re-seeds from `chain_hsn + 1` on the next registration (see the `NonceStore`
  docs in `storage/mod.rs`) — survivable, but a behavioural change that should
  be deliberate rather than accidental.
- Apply the [per-store PRAGMAs and pool config](03-configuration-guide.md#storage-provider--sqlite-wal-per-bucket).
- Add `--storage-mode sqlite` alongside the existing `inmemory` / `disk` modes in
  `provider-node/src/cli.rs` + `command.rs`. The old RocksDB `disk` mode stays
  available throughout.

### Phase 2 — data migration

- One-shot migrator: open the legacy RocksDB, iterate `CF_BUCKETS`; for each
  bucket create its two files, inserting leaves **by position** into the
  commitment store and nodes **by hash** into the content store; write `meta`. Idempotent and resumable (skip buckets whose file already
  exists and matches `leaf_count`).
- **Recompute `used_bytes`; do not copy it.** The legacy value is undercharged:
  `store_node` increments it only when the hash is absent from the *global*
  pool, so a bucket was never charged for content another bucket had already
  stored. Per-bucket stores change the semantics to "bytes this bucket holds",
  which is what quotas are supposed to enforce — so derive it from the nodes
  actually inserted into each bucket's content store. Copying the legacy number
  perpetuates the under-charge into the new layout. Note the asymmetry:
  `MmrLeaf.total_size` lives inside **signed** leaves and is immutable, so a
  bucket whose recomputed `used_bytes` disagrees with its leaves' `total_size`
  should be reported, never rewritten.
- Migrate `CF_METADATA`'s `KEY_NONCE` into `provider.sqlite` in the same pass.
  It is one row, and it is the only state that has no per-bucket destination.
- Verify per bucket by recomputing the **MMR root** from the migrated `leaves`
  and asserting it equals the on-chain checkpoint root before retiring the source.
- Providers can also **drain-and-refill** instead of migrating: stand up a new
  SQLite-backed provider and let the existing sync protocol replicate buckets —
  often simpler operationally than an offline migration.

### Phase 3 — cutover & cleanup

- Run new providers with `--storage-mode sqlite`; keep `disk` (RocksDB) selectable
  as rollback until the SQLite path has soaked in production.
- Once stable, mark RocksDB `disk` mode deprecated; remove the unused
  `CF_ROOT_TO_BUCKET` column family logic. Keep the `StorageBackend` trait and the
  in-memory backend (still useful for tests/dev).

---

## Sequencing

```
┌─ Storage (with Issue #100) ──────────────────────────────────────┐
│ Phase 1: SqliteBucketStore, two stores/bucket, LRU pool, barrier │
│ Phase 2: migrate / drain-and-refill, verify MMR roots            │
│ Phase 3: cutover, keep RocksDB as rollback, then deprecate       │
└──────────────────────────────────────────────────────────────────┘
```

## Risk & rollback

| Risk | Mitigation |
|------|-----------|
| New SQLite backend has a correctness bug | Trait-isolated; `--storage-mode disk` (RocksDB) stays selectable for instant rollback |
| Migration corrupts/loses data | Per-bucket MMR-root verification against on-chain checkpoint before retiring source; idempotent migrator |
| Too many open buckets exhaust FDs/RAM | LRU pool cap sized from the [config guide](03-configuration-guide.md). Budget **~6 FDs per hot bucket** — two stores × 3 FDs (main + `-wal` + `-shm`) — and ~144 KiB RSS at pass-4 disk figures |
| Absolute perf differs from tmpfs benchmark | Re-run `just db-bench` on target SSD hardware before final sign-off |

## Definition of done

- [ ] `SqliteBucketStore` implements `StorageBackend` with the two-store layout and per-position MMR storage; `--storage-mode sqlite` available.
- [ ] The ingest → barrier → commit → sign sequence lives in one function, shared by all three upload paths, with a crash-injection test.
- [ ] Content store uses the split hash index and `mmap_size`; a regression test asserts a dedup miss stays in single-digit µs on a populated bucket.
- [ ] All existing provider-node + client tests pass against the SQLite backend.
- [ ] Migration verified by MMR-root equality per bucket; `used_bytes` recomputed rather than copied, and `KEY_NONCE` landed in `provider.sqlite`.
- [ ] Layer-1 index persisted after the commitment commit inside the shared sequence, with startup reconciliation for dangling entries.
- [ ] RocksDB retained as documented rollback; deprecation tracked.
