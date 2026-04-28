# Web3 Storage — Hackathon Testing Guide

**Project**: Scalable Web3 Storage — TypeScript SDKs for decentralized S3-compatible and file system storage

**One-line**: Use the TypeScript SDKs to store and retrieve data programmatically, as a developer would when integrating storage into their own project.

**Branch**: Use the `prototyping-and-testing` branch for all testing:
```bash
git checkout prototyping-and-testing
```

## Known Issues

- **[#44](https://github.com/paritytech/web3-storage/issues/44)**: We are already aware of this issue — no need to report it again.

## Setup

1. Run `just setup` (one-time — downloads binaries, builds project)
2. `just start-chain` (Terminal 1 — starts relay chain + parachain)
3. `just start-provider` (Terminal 2 — starts storage provider node)
4. SDKs are at `user-interfaces/sdk/typescript/`

### SDK Setup

```bash
cd user-interfaces/sdk/typescript/s3          # or file-system
npm install
npm run papi:generate   # Generates chain type descriptors (requires chain running)
npm run build
```

## Test Scenarios

### Scenario 1: "S3 SDK — Store and retrieve objects" (30-45 min)

Using `@web3-storage/s3-sdk`:

```typescript
import { S3Client } from "@web3-storage/s3-sdk";

const client = new S3Client({
  chainWs: "ws://127.0.0.1:2222",
  providerUrl: "http://127.0.0.1:3333",
});

await client.connect();
await client.setSigner("//Alice");

// Create bucket (with automatic storage agreement)
const bucket = await client.createBucket("my-test-bucket", {
  capacity: 1_000_000_000n,        // 1 GB
  duration: 500,                    // 500 blocks
  maxPayment: 1_000_000_000_000_000n, // 1000 tokens
});

// Upload
const data = new TextEncoder().encode("Hello from hackathon!");
await client.putObject("my-test-bucket", "hello.txt", data, {
  contentType: "text/plain",
});

// Download and verify
const obj = await client.getObject("my-test-bucket", "hello.txt");
const text = new TextDecoder().decode(obj.data);
console.log(text); // "Hello from hackathon!"
```

Or run the included example directly:
```bash
cd user-interfaces/sdk/typescript/s3
npx tsx examples/basic-usage.ts
```

**What to test:**
- Create a bucket — how long does it take? Is feedback clear?
- Upload 5-10 objects with different keys and prefixes
- `listObjectsV2` with prefix filtering — does it return the right subset?
- `headObject` — does metadata match what you uploaded?
- `deleteObject` then GET — what error do you get?
- `copyObject` between buckets — does it work?

### Scenario 2: "File System SDK — Drives and files" (1-2 hours)

Using `@web3-storage/file-system-sdk`:

```typescript
import { FileSystemClient } from "@web3-storage/file-system-sdk";

const client = new FileSystemClient({
  chainWs: "ws://127.0.0.1:2222",
  providerUrl: "http://127.0.0.1:3333",
});

await client.connect();
await client.setSigner("//Alice");

// Create a drive
const driveId = await client.createDrive({
  name: "Hackathon Drive",
  capacity: 1_000_000_000n,        // 1 GB
  duration: 500,                    // 500 blocks
  maxPayment: 1_000_000_000_000_000n,
  minProviders: 1,
});

// Create directories
await client.createDirectory(driveId, "/docs");

// Upload a file
const content = new TextEncoder().encode("Hello from the file system!");
const result = await client.uploadFile(driveId, "/docs/hello.txt", content);

// Download by path
const downloaded = await client.downloadFile(driveId, "/docs/hello.txt");
console.log(new TextDecoder().decode(downloaded.data));

// List directory
const listing = await client.listDirectory(driveId, "/docs");
console.log(listing.entries);

// Delete a file
await client.deleteFile(driveId, "/docs/hello.txt");
```

Or run the included example:
```bash
cd user-interfaces/sdk/typescript/file-system
npx tsx examples/basic-usage.ts
```

**What to test:**
- Create a drive — is the parameter set (capacity, duration, payment) intuitive?
- `createDirectory` — create nested dirs, then `listDirectory` to verify
- `uploadFile` then `downloadFile` by path — does round-trip work?
- `listDirectory` — does it show files and dirs you created?
- `deleteFile` then `downloadFile` — what error do you get?
- `getIndexRoot` — does it reflect the current state of the drive?
- Upload the same file twice — does deduplication work?
- List drives — do they all show up?

### Scenario 3: "S3 HTTP API directly" (1 hour)

The provider node exposes a raw HTTP API. After creating a bucket via the SDK, try hitting the endpoints directly with curl or any HTTP client:

```bash
# Upload
curl -X PUT "http://127.0.0.1:3333/s3/{bucket_id}/object?key=test.txt" \
  -H "Content-Type: text/plain" \
  -d "hello world"

# Download
curl "http://127.0.0.1:3333/s3/{bucket_id}/object?key=test.txt"

# List objects
curl "http://127.0.0.1:3333/s3/{bucket_id}/objects?prefix=folder/"

# Delete
curl -X DELETE "http://127.0.0.1:3333/s3/{bucket_id}/object?key=test.txt"
```

**What to test:**
- Does the API behave like you'd expect from S3?
- What happens with auth headers missing?
- What do error responses look like?

### Scenario 4: "Explore the Web UIs" (30 min)

To get a feel for the full flow before diving into the SDKs, try the web-based consoles:

```bash
# Terminal 3: S3 Console UI (bucket/object management)
cd user-interfaces/console-ui
npm install && npm run dev

# Terminal 4: File System / Drive UI
cd user-interfaces/drive-ui
npm install && npm run dev
```

Use these UIs to create buckets/drives, upload files, and browse objects visually. This helps you understand the underlying flow before writing code against the SDKs.

### Scenario 5: "Break things" (ongoing)

- Upload a large file (50MB, 100MB, 500MB) — where does it break or slow down?
- Create a bucket with very small capacity, try to exceed it
- Kill the provider mid-upload, restart — what state is the bucket in?
- Create a bucket, don't wait for provider acceptance, try to upload immediately
- Use invalid bucket names (uppercase, special chars, too long) — are errors helpful?
- Call `getObject` on a key that doesn't exist — what error?

## Provider HTTP API Reference

All endpoints run on the provider node (default `http://127.0.0.1:3333`). Replace `{bucket_id}` with the Layer 0 bucket ID (a number).

### S3-Compatible Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| `PUT` | `/s3/{bucket_id}/object?key=<key>` | Upload an object |
| `GET` | `/s3/{bucket_id}/object?key=<key>` | Download an object |
| `HEAD` | `/s3/{bucket_id}/object?key=<key>` | Get object metadata |
| `DELETE` | `/s3/{bucket_id}/object?key=<key>` | Delete an object |
| `GET` | `/s3/{bucket_id}/objects?prefix=<prefix>&delimiter=<delim>&max_keys=<n>` | List objects |

```bash
# Upload
curl -X PUT "http://127.0.0.1:3333/s3/0/object?key=hello.txt" \
  -H "Content-Type: text/plain" \
  -d "hello world"

# Download
curl "http://127.0.0.1:3333/s3/0/object?key=hello.txt"

# Metadata
curl -I "http://127.0.0.1:3333/s3/0/object?key=hello.txt"

# List (with prefix)
curl "http://127.0.0.1:3333/s3/0/objects?prefix=data/"

# Delete
curl -X DELETE "http://127.0.0.1:3333/s3/0/object?key=hello.txt"
```

### File System Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| `PUT` | `/fs/{bucket_id}/file?path=<path>` | Upload a file (binary body) |
| `GET` | `/fs/{bucket_id}/file?path=<path>` | Download a file |
| `DELETE` | `/fs/{bucket_id}/file?path=<path>` | Delete a file |
| `POST` | `/fs/{bucket_id}/mkdir?path=<path>` | Create a directory |
| `GET` | `/fs/{bucket_id}/ls?path=<path>&recursive=false` | List directory contents |
| `GET` | `/fs/{bucket_id}/index_root` | Get drive index/integrity info |

```bash
# Create directory
curl -X POST "http://127.0.0.1:3333/fs/0/mkdir?path=/documents"

# Upload file
curl -X PUT "http://127.0.0.1:3333/fs/0/file?path=/documents/hello.txt" \
  -H "Content-Type: text/plain" \
  -d "hello world"

# Download file
curl "http://127.0.0.1:3333/fs/0/file?path=/documents/hello.txt"

# List directory
curl "http://127.0.0.1:3333/fs/0/ls?path=/documents"

# List recursively
curl "http://127.0.0.1:3333/fs/0/ls?path=/&recursive=true"

# Get index root
curl "http://127.0.0.1:3333/fs/0/index_root"

# Delete file
curl -X DELETE "http://127.0.0.1:3333/fs/0/file?path=/documents/hello.txt"
```

### Common Responses

**Upload (PUT /fs/.../file)**: `{ "data_root": "0x...", "size": 1234, "leaf_index": 0 }`

**List directory (GET /fs/.../ls)**: `{ "path": "/documents", "entries": [{ "name": "hello.txt", "path": "/documents/hello.txt", "entry_type": "file", "size": 11, "mtime": 1711234567 }], "file_count": 1, "dir_count": 0, "total_size": 11 }`

**Index root (GET /fs/.../index_root)**: `{ "metadata_merkle_root": "0x...", "file_count": 5, "dir_count": 2, "total_size": 12345 }`

**Delete (DELETE /fs/.../file)**: `{ "deleted": true }`

**Create directory (POST /fs/.../mkdir)**: `{ "path": "/documents", "created": true }`

## What We Want to Learn

| Question | Why it matters |
|----------|---------------|
| Can a developer go from `git clone` to "data stored" using only the SDK docs? | Developer experience is the product |
| Does `npm install` + `papi:generate` + `npm run example` work without issues? | First-run experience |
| Are the SDK's error messages actionable? | "BucketNotFound" is useful, "Error(3)" is not |
| Is the bucket creation → upload → download flow obvious from the API? | Critical onboarding path |
| What methods or types are missing that you expected to exist? | API surface gaps |
| Where did you read source code because the docs weren't enough? | Documentation gaps |
| What's the largest file size that works reliably? | We need real numbers |
| S3 SDK vs File System SDK — which felt more natural for your use case? | Helps us prioritize |

## Materials

- [`user-interfaces/sdk/typescript/README.md`](../../user-interfaces/sdk/typescript/README.md) — SDK overview and quick start
- [`user-interfaces/sdk/typescript/s3/README.md`](../../user-interfaces/sdk/typescript/s3/README.md) — S3 SDK API reference
- [`user-interfaces/sdk/typescript/file-system/README.md`](../../user-interfaces/sdk/typescript/file-system/README.md) — File System SDK API reference
- [`CLAUDE.md`](../../CLAUDE.md) — all build/test/run commands for infrastructure
- [`docs/getting-started/QUICKSTART.md`](../getting-started/QUICKSTART.md) — infrastructure setup

Pre-built binaries can be provided so testers skip the ~10 min release build for the chain and provider.

## Feedback Format

For each scenario attempted:

1. **What you tried** (code snippet or steps)
2. **Where you got stuck** (exact error, confusing API, missing docs)
3. **Time spent** (wall clock)
4. **Severity**: Blocker / Annoying / Minor / Suggestion
5. **Code you wish existed** (e.g. "I wanted `client.uploadFile(path)` but had to read the source to figure out the CID flow")
