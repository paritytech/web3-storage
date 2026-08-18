# Migration Plan

From today's **single shared RocksDB** to the recommended architecture:
**SQLite per bucket** in the Storage Provider.
Sequenced to interleave with [Issue #100](https://github.com/paritytech/web3-storage/issues/100)
(per-bucket isolation), which this evaluation directly informs.

## Starting point

- **Storage Provider** — `provider-node/src/storage/disk.rs`: one `rocksdb::DB`
  with three column families (`CF_NODES`, `CF_BUCKETS`, `CF_ROOT_TO_BUCKET`).
  Critically, `BucketState` serializes **all MMR leaves into a single value**, so
  every commit/proof/delete is O(n) on the whole leaf set — the core problem
  Issue #100 targets. The backend already sits behind the `StorageBackend` trait
  (`provider-node/src/storage/mod.rs`), selected at startup via `--storage-mode`.

The `StorageBackend` trait is the seam that makes the provider migration low-risk:
a new backend is a new trait impl, switchable by flag, with no callers changed.

---

## Storage Provider migration (couples with Issue #100)

The engine choice (SQLite) and the per-bucket isolation (Issue #100) are one
change: SQLite is *chosen because* of the per-bucket model, so implement them
together. The architecture decision itself is now evidence-backed — see the
sharded-vs-shared matrix in [report 01](01-storage-provider-benchmark.md).

### Phase 1 — `SqliteBucketStore` behind the existing trait

- Add a new `StorageBackend` implementation (e.g.
  `provider-node/src/storage/sqlite_bucket.rs`) that owns an **LRU pool of
  per-bucket SQLite connections** instead of one shared DB. One file per bucket:
  `{storage_path}/buckets/{bucket_id}.sqlite`.
- **Schema** (resolves the O(n) blob): store MMR leaves **per position**, not as
  one serialized vector —

  ```sql
  CREATE TABLE leaves (pos INTEGER PRIMARY KEY, leaf BLOB NOT NULL) WITHOUT ROWID;
  CREATE TABLE meta   (k TEXT PRIMARY KEY, v BLOB NOT NULL);  -- mmr_root, start_seq, used/max bytes
  CREATE TABLE nodes  (hash BLOB PRIMARY KEY, data BLOB NOT NULL, children BLOB);
  ```

  `commit`, `delete_before`, and proof reads become **bounded range operations**
  on `leaves` keyed by position, instead of deserialize-mutate-reserialize of the
  whole set.
- Apply the [PRAGMAs and pool config](03-configuration-guide.md#storage-provider--sqlite-wal-per-bucket).
- Add `--storage-mode sqlite` alongside the existing `inmemory` / `disk` modes in
  `provider-node/src/cli.rs` + `command.rs`. The old RocksDB `disk` mode stays
  available throughout.

### Phase 2 — data migration

- One-shot migrator: open the legacy RocksDB, iterate `CF_BUCKETS`; for each
  bucket create its `.sqlite` file and insert leaves **by position** and nodes by
  hash; write `meta`. Idempotent and resumable (skip buckets whose file already
  exists and matches `leaf_count`).
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
│ Phase 1: SqliteBucketStore + per-position schema + LRU pool      │
│ Phase 2: migrate / drain-and-refill, verify MMR roots            │
│ Phase 3: cutover, keep RocksDB as rollback, then deprecate       │
└──────────────────────────────────────────────────────────────────┘
```

## Risk & rollback

| Risk | Mitigation |
|------|-----------|
| New SQLite backend has a correctness bug | Trait-isolated; `--storage-mode disk` (RocksDB) stays selectable for instant rollback |
| Migration corrupts/loses data | Per-bucket MMR-root verification against on-chain checkpoint before retiring source; idempotent migrator |
| Too many open buckets exhaust FDs/RAM | LRU pool cap sized from the [config guide](03-configuration-guide.md); SQLite's 3 FDs + 32 KiB per instance give large headroom |
| Absolute perf differs from tmpfs benchmark | Re-run `just db-bench` on target SSD hardware before final sign-off |

## Definition of done

- [ ] `SqliteBucketStore` implements `StorageBackend` with per-position MMR storage; `--storage-mode sqlite` available.
- [ ] All existing provider-node + client tests pass against the SQLite backend.
- [ ] Migration verified by MMR-root equality per bucket.
- [ ] RocksDB retained as documented rollback; deprecation tracked.
