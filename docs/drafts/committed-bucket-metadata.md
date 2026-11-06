# RFC: Committed Bucket Metadata

|                 |                   |
| --------------- | ----------------- |
| **Start Date**  | 2026-09-10        |
| **Description** | Make a bucket's name→content mapping (the Layer-1 metadata) committed, verifiable data in the bucket itself, anchored by a checkpoint-carried pointer — specified generically, applied to S3 first. |
| **Authors**     | Naren Mudigal     |
| **Status**      | Draft — unratified; lives in `docs/drafts/` until design review promotes it |
| **Relates to**  | [#253 discussion on per-bucket stores](https://github.com/paritytech/web3-storage/pull/253), [#344 client-protocol clarifications](https://github.com/paritytech/web3-storage/pull/344), `docs/design/S3_METADATA_INDEX.md` (current derived index), the canonical MMR window semantics (`docs/design/scalable-web3-storage.md`, "MMR Commitments") |

## Summary

Layer 0 commits and slashes over *content*, but the mapping from human-readable
names to that content — an S3 object key to its `data_root` — lives today only
in a provider-local index (`s3_indices/bucket_N_index.json`). A provider can
drop or rewind that mapping with no on-chain consequence, and a cold-start
client (one recovering with no local state) cannot even detect it.

This RFC makes the metadata a first-class committed object:

1. **Metadata versions are committed blobs.** Each metadata update writes a new
   version of an authenticated search tree (MST) into the bucket, structurally
   sharing unchanged nodes with the previous version and linking it via `prev`
   — the Git model: a chain of complete states, not a log of edits.
2. **The current version is anchored in the checkpoint.** The checkpoint
   payload gains one opaque field, `app_root: Option<H256>`, signed by the
   primaries together with `mmr_root` and `leaf_count`. Pointer and liability
   are atomic by construction.
3. **Retention and compaction are per-bucket policy** riding the existing
   deletion/compaction machinery: S3 buckets default to retain-nothing
   (overwrite reclaims, matching S3 semantics); reclamation is finalized by the
   next checkpoint, never by a separate coordination round.

The contract is generic (any Layer-1 personality supplies its own metadata
shape); this RFC fully specifies only the **S3** application. The file-system
layer already implements the same pattern implicitly (its `DirectoryNode` DAG
*is* committed metadata); aligning it, and a future document store, are future
work.

## Motivation

### The attack surface today

Content is protected end to end: chunks hash into a `data_root`, `data_root`s
are MMR leaves, the MMR root is checkpointed on-chain with primary signatures,
and loss is slashable via challenges. The *name* of the content is not:
`s3_put_object` commits only the `data_root` and writes the
`key → ObjectMeta{data_root, …}` pair solely into the provider's local index
(`provider-node/src/s3_api.rs`; index in
`crates/providers/storage/src/index/s3.rs`).

Three consequences, none punishable today:

- **Denial by name.** Delete the index entry: the bytes remain (challenges are
  defended, nothing slashes), but a user who knows only the key cannot reach
  them.
- **Rollback.** Point the key at an older version's `data_root`. Both versions
  are validly committed leaves; every proof verifies; the client gets stale
  data with green checkmarks.
- **Cold-start blindness.** The existing `metadata_merkle_root` endpoint lets a
  client *detect* a changed index only if it remembers a prior reference. A
  client recovering from key-only state must trust whatever index the provider
  serves.

The FS layer avoids all three for its tree structure (the DAG is committed
data) but shares a fourth gap: the pointer to the *current* root CID is managed
off-chain only, so serving an old root replays an entire old drive, validly.

### Why S3 first

A Layer-0 bucket by itself is a disk with hash addresses — verifiable, and
not an interface humans use. People name things, and for named object storage
the S3 API is the de facto standard: its vocabulary (buckets, keys, `PUT`/`GET`,
prefix listing) is what developers already know from AWS, and a large tooling
ecosystem (SDKs, `rclone`, CI pipelines, backup tools) speaks it unmodified.
This repo already ships the S3 personality (`s3-registry`, the provider's
`/s3` API, `s3-ui`), so it is the surface real users touch first — which is
exactly why it leads this RFC: the interface most people will use should not
be the least-protected layer of the system, and today the S3 key mapping is
the only link in the read path with no on-chain backing.

### Why now

The per-bucket store work (#253) fixes the provider's local layout, and its
review raised exactly this question (which cross-bucket/global metadata is
authoritative). The answer should be settled before the storage backend
migration bakes in the current trust assumptions.

## Stakeholders

- **Storage team** — protocol change (checkpoint payload), provider node, SDKs.
- **S3 interface consumers** — semantics of overwrite/delete/list change from
  trusted-index to verifiable.
- **#253 (storage backend)** — the metadata blobs are ordinary nodes in the
  per-bucket content store; layout is untouched, but the local index formally
  becomes a cache.
- **#344 (client protocol)** — compaction ordering and the extension-proof
  interaction are specified against that PR's normative additions.

## Explanation

### Terminology

- **Metadata (version)** — the committed blob (or blob-DAG) describing a
  bucket's current name→content state. Versions are labeled M1, M2, … in
  examples. Distinct from the on-chain `BucketSnapshot`, which this RFC never
  renames.
- **`app_root`** — the CID of the current metadata version, carried in the
  checkpoint.
- **Personality** — the single Layer-1 interface a bucket serves (S3 bucket,
  FS drive, future document store). One bucket has exactly one personality.

### The generic contract

The blob behind `app_root` MUST be:

1. **Committed** — content-addressed data in the bucket: same chunking, same
   blake2-256, same MMR commitment and challenge mechanics as any content.
2. **Layer-shaped** — its schema fixed by the bucket's personality.
3. **Canonical** — the same logical state serializes to byte-identical form
   (hence the same CID) regardless of implementation. Requires a normative
   encoding spec with shared test vectors (see Unresolved Questions).

Metadata blobs carry a fixed magic prefix and version byte — a parse-time
sanity check, not a discovery mechanism: metadata is only ever found through
the checkpoint's `app_root` and each version's `prev`/child links, never by
scanning leaves. Raw untyped leaves remain fully supported: the canonical
design's client-controlled layout lets a bucket commit arbitrary blobs with
no metadata at all.

### S3 metadata structure

An authenticated sorted map — Merkle Search Tree (MST) / Prolly-tree family —
over object keys:

```
MetadataVersion {
    magic:   [u8; 4],          // b"W3SM"
    version: u8,               // encoding version
    kind:    u16,              // 1 = s3-index; open registry — readers must
                               // report unknown kinds, not fail to parse
    prev:    Option<H256>,     // CID of the previous metadata version
    root:    MstNode,          // by value or by CID, per encoding spec
}

MstNode {
    entries:  [(key: Bytes, value: ObjectMeta | child: H256)],  // sorted by key
}

ObjectMeta {
    data_root:      H256,      // the object's chunk-tree root
    size:           u64,
    content_type:   Bytes,
    last_modified:  u64,
    user_metadata:  [(Bytes, Bytes)],   // bounded
}
```

Metadata maps names to CIDs; it never embeds content. An `ObjectMeta` is
~100 bytes regardless of object size — which is what makes versions cheap,
keeps content dedup independent of naming (one chunk tree, any number of
keys pointing at it), and makes rename O(log n) for a file of any size.

Properties this buys, each load-bearing for S3 semantics:

- **Key uniqueness by construction** — a sorted map has one slot per key, so
  a later `PUT` replaces (S3's own `PUT` contract) and duplicates are
  unrepresentable.
- **Prefix listing as a range scan** — `ListObjectsV2` over a Merkle proof.
- **Absence proofs** — a provable 404 via sorted adjacency.
- **Structural sharing** — one key update creates ~log n new nodes (new leaf,
  new path, new root); all untouched nodes are referenced, not copied, and the
  content store's dedup stores them once across versions. This is what makes a
  metadata version per change affordable; a monolithic document (today's JSON
  index shape) would recommit nearly the whole index per `PUT`. Concretely: a
  100k-key bucket (fanout ~32 → depth ~4, index ~10 MB materialized) pays
  ~4 new nodes ≈ 10–20 KB per update — 0.1–0.2% of the index — so a thousand
  updates cost ~10–20 MB of version history in total, not a thousand copies.

### Write path (single writer)

Per `PUT key`:

1. Upload/dedup the object's chunks; build its chunk tree → `data_root`.
2. Build metadata version M(n+1) from M(n): new MST path for `key`,
   `prev = CID(M(n))`; upload its new nodes (dedup makes this ~log n blobs).
3. Commit both as MMR leaves: the content `data_root`, then `CID(M(n+1))`.
4. The provider-signed commitment covers both immediately (pre-checkpoint
   protection, as for any write).

`DELETE key` is the same with an MST removal. The provider's local index
(`s3_indices/…`, or its SQLite successor under #253) is maintained as before
but is henceforth **a cache of the committed metadata, never authoritative**:
any party can rebuild and verify it.

### Anchoring: the checkpoint carries `app_root`

The checkpoint's signed payload (`CommitmentPayload`) gains one opaque field:

```
app_root: Option<H256>   // CID of the current metadata version — a leaf in the window
```

Validity rule: `app_root` MUST be the CID of a leaf within the checkpointed
window `[start_seq, start_seq + leaf_count)`. Nothing enforces "newest" — the
pointer itself defines what is current (the owner assembles the checkpoint;
pinning an older version misleads only readers of their own bucket, no more
than never writing the new version would). Layer 0 stores and compares
`app_root` as opaque bytes; it never interprets the blob.

Why this placement (alternatives in a later section): it adds **no new
extrinsic and no marginal fee** (it rides a transaction that already happens);
it is **atomic with liability** — the same primary signatures that pin
`leaf_count` pin the pointer, so `app_root` can never name a blob outside the
slashable window (the dangling-pointer bug is unwritable); and the reader
resolves it in the **one chain read they already perform** (`bucket_info`'s
snapshot).

The residual freshness bound is the system's own: between checkpoints the chain
names the previous metadata version — a stale-but-complete view, identical to
the freshness the bytes themselves have. Clients holding the provider-signed
commitment for newer writes retain pre-checkpoint protection as today.

### Read path

- **Hot path**: unchanged — ask the provider by key; it answers from its cache.
- **Verifying client**: fetch `bucket_info` → snapshot `{mmr_root, leaf_count,
  app_root}`; fetch the metadata version at `app_root` (MMR-proof it into the
  window); walk the MST for the key (or its absence); fetch chunks under the
  proven `data_root`. Every arrow is a hash.
- **Cold start**: identical — the chain supplies the trust root; nothing is
  taken on the provider's word.

### Retention, deletion, compaction

Mechanism (unchanged from #344's pruning rules, restated): compaction
re-commits the survivor set as a fresh window — the current metadata version
first (its CID, and therefore `app_root`, is unchanged), then live content
roots; the `start_seq` advance rides the next checkpoint; physical discard only
after that checkpoint is final **and** open challenges against the old window
are resolved.

Scheduling (new, policy not protocol):

- **Explicit delete** → mark; fold into the next checkpoint's compaction.
- **Overwrite** → per-personality retention default. **S3 defaults to
  retain-nothing**: S3's own contract is that overwriting a key reuses the
  space, so old versions are garbage the moment they are superseded. (A
  retention depth k / duration t is a per-bucket setting for buckets that want
  it.)
- **Quota pressure** → forced compaction when `used_bytes` crosses a threshold
  of `max_bytes`, since reclaiming quota is the only economic value compaction
  has to the owner (fees price `max_bytes × duration` upfront; garbage is sunk
  cost until headroom runs out).
- **Never** unconditionally per checkpoint, and never as a standalone
  coordination round: per-checkpoint compaction would renumber every leaf every
  time, forfeiting MMR append-only incrementality (replicas re-walk O(n)
  instead of appending) and making #344's append-only extension proof
  unavailable between any two anchors.

A bucket under S3's retain-nothing default converges to exactly one metadata
version plus live content — "latest-metadata-only" is the compacted steady
state, not a rival design. Two notes on the version chain under this default:
even retain-nothing always holds **current + in-transition** — the previous
version must survive until the checkpoint naming its successor is final, since
until then it *is* the checkpointed, challengeable state; and after compaction
a version's `prev` may name a discarded CID. A dangling `prev` is harmless and
expected (like the cut-off parent in a shallow Git clone) — a resolvable
`prev`-chain is a retention feature, never a guarantee.

### Worked lifecycle

| Step | Leaves appended | Blobs created | Chain |
|---|---|---|---|
| PUT `a.jpg` (A) | `0xA1`, `0xM1` | chunk tree A1; M1 = {a→A1}, prev — | — |
| PUT `b.png` (B) | `0xB1`, `0xM2` | chunk tree B1; M2 root → NA(a→A1), NB(b→B1), prev M1 | — |
| Checkpoint #1 | — | — | `{count: 4, app_root: 0xM2}` |
| PUT `a.jpg` (A′) | `0xA2`, `0xM3` | chunk tree A2; M3 root → NA′(a→A2), **NB shared**, prev M2 | — |
| Checkpoint #2 | — | — | `{count: 6, app_root: 0xM3}` |
| Delete old versions → compaction | new window `[0xM3, 0xB1, 0xA2]` | none (re-commit only) | `{count: 3, app_root: 0xM3}` — unchanged CID |
| After finality | — | discard A1, M1, M2+NA (NB survives — M3 references it) | — |

## Drawbacks

- **Consensus-visible format change.** The checkpoint payload and its signature
  format change (+33 bytes). Fine pre-launch; requires gated design review and
  coordinated provider/SDK/pallet rollout.
- **Quota cost of history.** Metadata versions and superseded content count
  against `used_bytes` until compacted. Metadata itself is noise (~KBs); old
  content versions are the real cost, governed by retention policy.
- **Canonical-encoding burden.** The MST construction (split points, ordering,
  integer encodings) must be specified so independent implementations produce
  byte-identical versions — the same class of obligation as the currently
  unspecified `data_root` algorithm, and best solved in the same normative
  spec with shared test vectors.
- **Compaction boundaries break extension proofs.** Unavoidable (any
  renumbering does); mitigated by thresholded scheduling, and clients
  re-anchor on the post-compaction checkpoint.

## Testing, Security, Privacy

**Closed by this RFC:** name denial (the mapping's loss is provable,
challengeable data loss), rollback (the checkpoint names the current version;
serving M(n−1) contradicts chain state), cold-start trust (the chain, not the
provider, supplies the current index).

**Explicitly not closed:** freshness beyond checkpoint cadence (uniform with
data freshness; only per-read signed receipts would improve it); the global
`/exists` probe oracle (orthogonal; tracked with the Layer-0 read-gate work,
#396); key
visibility (metadata blobs are committed bucket content, readable by whoever
can read the bucket — for private buckets that is members, matching today's
listing surface; client-side encryption of keys composes on top, at the price
of server-side prefix queries).

**Tests:** encoding test vectors (empty map; one key; key at an MST split
boundary; unicode keys); rebuild-and-compare (fold the window, compare to
`app_root`) as a provider self-check and an SDK audit call; compaction
crash-ordering (extends #253's crash-injection obligations); challenge against
a metadata leaf (it must be defensible exactly like content).

## Performance, Ergonomics, Compatibility

- **Per `PUT`:** ~log n small metadata blobs (+1 MMR leaf) on top of today's
  writes; one extra small-blob fetch on a verifying read. Hot-path reads via
  the provider index are unchanged.
- **Provider:** the #253 per-bucket layout is untouched — metadata nodes are
  ordinary content-store rows; the local index becomes a rebuildable cache
  (startup scrub can verify it against `app_root`).
- **Compatibility:** a bucket that never writes metadata has no metadata
  leaves and no `app_root` (`None`) — behavior is exactly today's; adoption is
  per-bucket and incremental. The S3 API surface is unchanged;
  `metadata_merkle_root` is superseded by `app_root` verification and can be
  removed once this design lands.

## Alternatives considered

1. **Status quo (provider-local index + client-side `metadata_merkle_root`
   check).** Detectable only by clients with memory; never punishable;
   cold-start blind. Rejected as the motivating gap.
2. **Pointer as a Layer-0 bucket field with a new extrinsic.** Works, but
   costs a fee per update (so real apps batch to checkpoint cadence anyway —
   the same freshness as this RFC, minus the atomicity), leaks app semantics
   into L0's bucket struct, and creates the dangling-pointer ordering footgun
   (pointer updatable ahead of the covering checkpoint).
3. **Pointer in each Layer-1 registry (restore drive-registry's removed root
   tracking; add the same to s3-registry).** Cleanest layering, but N pallets
   re-implement the same update/gating/ordering rules with drift risk, readers
   pay two chain reads, and the fee-per-update economics are unchanged — the
   churn/cost reasons the drive registry removed root tracking apply again.
4. **Per-operation envelope log (self-describing typed leaves).** Strictly
   more general: ops merge under concurrent writers and need no prior state to
   append. Deliberately deferred, not rejected — it is the documented
   escalation path if multi-writer buckets (#344's drafts) are ratified; the
   Git-model state chain is simpler and sufficient for the single-writer
   architecture, and the two compose (ops between metadata versions).
5. **Monolithic index blob per change.** O(index) rewrite per `PUT` under fixed
   chunking; rejected on write amplification — the MST's structural sharing is
   what makes versioning affordable.

## Prior art

- **Git** — chains of complete, structurally-shared tree states with parent
  links; history by diffing states.
- **LSM / WAL databases** — base + tail, thresholded compaction, never
  compact-per-flush.
- **Prolly trees / Merkle Search Trees** — Dolt, Noms, Bluesky ATProto:
  history-independent authenticated maps with structural sharing and range
  proofs.
- **Certificate Transparency** — append-only logs with consistency
  (extension) proofs; the property preserved between compactions.
- **This repo** — the FS layer's `DirectoryNode` DAG (committed tree-shaped
  metadata, missing only the anchored current pointer) and
  `metadata_merkle_root` (the detection-only ancestor of `app_root`).

## Unresolved questions

1. **MST algorithm + canonical encoding**: exact split/probability function,
   node size bounds, encoding — one normative spec with test vectors, shared
   with (and motivating) the `data_root` construction spec.
2. **Where the `kind` registry lives** (`storage-primitives` constants vs a
   design-doc table) and its governance.
3. **Should primaries validate `app_root`** (is it a leaf in the window?)
   before signing a checkpoint, or sign it opaquely? Validation is cheap —
   a lookup in the provider's own MMR, no blob parsing — and closes a
   typo/stale-pointer error class; the cost is making L0 providers aware that
   the field references a leaf at all.
4. **Retention policy surface**: where per-bucket retention (depth k /
   duration t) is configured — on-chain bucket field vs off-chain SDK
   convention.
5. **Quota shrink gap** (exposed by the economics, out of scope here): no
   protocol path reduces `max_bytes` or recovers value for permanently
   shrunk buckets.
6. **Challenge pinning across compaction**: make explicit (in #344's pruning
   section or here) that open challenges against the old window defer physical
   discard, alongside checkpoint finality.

## Future directions

- **File system**: adopt `app_root` for the drive's current root CID
  (replacing the off-chain-only pointer) — the DAG side needs nothing else.
- **Document store**: MST over document ids; falls out of the contract.
- **Concurrent writers**: the envelope op-log as the merge substrate between
  metadata versions, composed with #344's writer-coordination drafts.
- **Key privacy**: encrypted-key MST variants for private buckets wanting
  hidden names with server-side range queries.
