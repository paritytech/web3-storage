# Content-Defined Chunking (CDC)

## Why

The provider stores file bytes as content-addressed chunks: each chunk's key is `blake2_256(bytes)`, so two PUTs that produce a byte-identical chunk get stored once. That dedup is automatic — *if* the chunker actually produces identical chunks across edits.

With a fixed-size chunker, it doesn't. Inserting a single byte at the start of a file shifts every downstream byte by one, so every chunk has different bytes, so every chunk has a different hash. v2's PUT effectively re-uploads the entire file.

Content-defined chunking solves this by choosing chunk boundaries from the *content* (via a rolling hash), not from fixed byte offsets. When you insert bytes in the middle of a file, the boundaries downstream of the edit fall at the same content positions as before; the chunks between them are byte-identical to v1's chunks and dedup through content addressing.

## Algorithm

[FastCDC](https://www.usenix.org/system/files/conference/atc16/atc16-paper-xia.pdf), via the [`fastcdc-rs`](https://crates.io/crates/fastcdc) crate. Picked over Rabin fingerprinting for:
- ~3-5× higher throughput (gear-hash based)
- Equivalent dedup ratio
- Maintained Rust implementation in production use by `restic` and others

Mechanically: slide a 32-byte window across the input, compute a gear hash of the window contents at each position, emit a boundary when the low bits of the hash match a fixed pattern (forced boundaries enforce `max_size`; the hash check is skipped below `min_size`).

## Parameters

| Parameter | Value | Why |
|---|---|---|
| `CDC_MIN_SIZE` | 64 KiB | Avoid tiny chunks that inflate metadata / proof overhead |
| `CDC_AVG_SIZE` | 256 KiB | Matches the prior `DEFAULT_CHUNK_SIZE` so MMR leaf counts and proof depths stay comparable |
| `CDC_MAX_SIZE` | 1 MiB | Bounds worst-case proof and read cost |
| Window size | 32 bytes (FastCDC default) | |

These live in [`primitives/src/chunking.rs`](../../primitives/src/chunking.rs).

## What changes, what doesn't

**Changed.** Chunks are now variable-size (between 64 KiB and 1 MiB). The provider's S3 (`PUT /s3/.../object`) and FS (`PUT /fs/.../file`) handlers chunk via `chunk_cdc_borrowed`. The Rust client SDK's `ChunkingStrategy::ContentDefined` arm uses the same chunker (previously a TODO that fell back to fixed-256K).

**Unchanged.** The MMR is still leaf-per-chunk and chunk-size-agnostic — each leaf is `blake2_256(chunk_bytes)`. The Merkle proof model is unchanged. The S3/FS GET handlers still reassemble via `collect_chunks(data_root)`, which walks the tree by hash and doesn't assume any chunk size. Content addressing and per-bucket isolation are unchanged.

The `GET /read?data_root=&offset=&length=` byte-range endpoint **still assumes fixed-size** (its arithmetic computes `chunk_index = offset / DEFAULT_CHUNK_SIZE`). It is therefore unsafe to call on CDC-chunked roots. For whole-file historical fetch, use the new `GET /content?data_root=` endpoint, which walks the manifest. Updating `/read` for variable-size byte-range reads is deferred until a caller actually needs it.

## Fixed vs CDC trade-offs

| | Fixed-size | CDC |
|---|---|---|
| Implementation | Trivial | FastCDC (~200 LOC dep) |
| Throughput | Memory-bandwidth-bound | ~1 GB/s on a modern CPU |
| Chunk-size variance | Zero (last chunk smaller) | Bounded by `[min, max]` |
| Insert/delete dedup | ❌ Cascades downstream | ✅ Only chunks straddling the edit change |
| In-place overwrite dedup | ✅ Works | ✅ Works |
| Append dedup | ✅ Works (only trailing chunk changes) | ✅ Works |
| Best for | Binary blobs with fixed-offset fields; embedded use where every byte counts | Text, structured data, anything with shifting edits |

`ChunkingStrategy::Fixed(usize)` remains the SDK default; CDC is opt-in via `ChunkingStrategy::ContentDefined`. The provider's HTTP upload endpoints (S3 / FS) always use CDC since their callers don't pick a strategy.

## Verification

- Unit tests in `primitives/src/chunking.rs::tests` cover determinism, size-distribution bounds, byte-equal reassembly, and ≥ 90% chunk reuse on mid-file insertion / deletion of 8 MiB random data.
- Integration test `test_s3_cdc_dedup_across_versions` in `provider-node/tests/s3_integration.rs` PUTs an 8 MiB blob, then a mid-file-edited variant, and asserts the second PUT adds fewer than `total_nodes(v1) / 4` new nodes.
- `test_get_content_returns_full_bytes` exercises the new `/content` endpoint.
