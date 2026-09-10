# Layer 1 — File System & S3 Interfaces (Design / Implementation)

> **Draft — needs triage.** Extracted from the Layer 0 implementation design
> doc. Promote into `design/` or drop — pending
> the layered-architecture decision in
> [#51](https://github.com/paritytech/web3-storage/issues/51) (what belongs
> on-chain vs. provider-only).

The provider node also serves the Layer 1 interfaces described in
the `file-system-client` and `s3-client` crates and [`./marketplace.md`](./marketplace.md). These mount on
top of the Layer 0 blob primitives and require Writer/Admin authorization for
mutating routes.

```
S3-compatible object storage
────────────────────────────
PUT    /s3/:bucket_id/object?key=path/to/object
GET    /s3/:bucket_id/object?key=path/to/object
HEAD   /s3/:bucket_id/object?key=path/to/object
DELETE /s3/:bucket_id/object?key=path/to/object
GET    /s3/:bucket_id/objects                     # list
GET    /s3/:bucket_id/index_root                  # current S3 index CID

File system interface
─────────────────────
PUT    /fs/:bucket_id/file?path=/dir/file.txt
GET    /fs/:bucket_id/file?path=/dir/file.txt
DELETE /fs/:bucket_id/file?path=/dir/file.txt
POST   /fs/:bucket_id/mkdir                       # body: { path: ... }
GET    /fs/:bucket_id/ls?path=/dir
GET    /fs/:bucket_id/index_root                  # current drive root CID
```

The request/response shapes, error codes, and manifest formats are defined in
the `file-system-primitives` crate and the provider node; see their rustdoc.
