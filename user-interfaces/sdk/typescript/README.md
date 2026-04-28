# Web3 Storage TypeScript SDKs

TypeScript/JavaScript SDKs for the Web3 Storage Layer 1 interfaces.

## Available SDKs

| SDK | Package | Description |
|-----|---------|-------------|
| [File System](./file-system/) | `@web3-storage/file-system-sdk` | Familiar file/folder operations |
| [S3](./s3/) | `@web3-storage/s3-sdk` | S3-compatible object storage |

## Quick Start

### Prerequisites

1. **Running Infrastructure**
   ```bash
   # Terminal 1: Start blockchain
   just start-chain

   # Terminal 2: Start provider
   just start-provider
   ```

2. **Node.js 18+**

### File System SDK

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
  name: "My Drive",
  capacity: 10_000_000_000n,
  duration: 500,
  maxPayment: 1_000_000_000_000_000n,
});

// Upload a file
const content = new TextEncoder().encode("Hello!");
await client.uploadFile(driveId, "/hello.txt", content);
```

### S3 SDK

```typescript
import { S3Client } from "@web3-storage/s3-sdk";

const client = new S3Client({
  chainWs: "ws://127.0.0.1:2222",
  providerUrl: "http://127.0.0.1:3333",
});

await client.connect();
await client.setSigner("//Alice");

// Create a bucket
await client.createBucket("my-bucket");

// Upload an object
const data = new TextEncoder().encode("Hello!");
await client.putObject("my-bucket", "hello.txt", data);

// Download an object
const obj = await client.getObject("my-bucket", "hello.txt");
console.log(new TextDecoder().decode(obj.data));
```

## Development Setup

Each SDK requires chain type descriptors to be generated from a running parachain.

```bash
cd sdk/typescript/file-system  # or sdk/typescript/s3
npm install
npm run papi:generate  # Requires parachain running at ws://localhost:2222
npm run build
npm run example
```

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Your Application                            │
└───────────────┬─────────────────────────────┬───────────────────┘
                │                             │
                ▼                             ▼
┌───────────────────────────┐   ┌───────────────────────────────┐
│  @web3-storage/           │   │  @web3-storage/               │
│  file-system-sdk          │   │  s3-sdk                       │
└───────────────┬───────────┘   └───────────────┬───────────────┘
                │                               │
                └───────────────┬───────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                         polkadot-api                            │
│                    (Chain interaction)                          │
└───────────────────────────────┬─────────────────────────────────┘
                                │
                ┌───────────────┴───────────────┐
                │                               │
                ▼                               ▼
┌───────────────────────────┐   ┌───────────────────────────────┐
│   Parachain (Layer 0+1)   │   │     Provider Node             │
│   - DriveRegistry         │   │     - Data storage            │
│   - S3Registry            │   │     - HTTP API                │
│   - StorageProvider       │   │     - MMR commitments         │
└───────────────────────────┘   └───────────────────────────────┘
```

## File System vs S3: When to Use Which

| Use Case | Recommended SDK |
|----------|----------------|
| Hierarchical file organization | File System |
| AWS S3 compatibility | S3 |
| Simple key-value storage | S3 |
| Complex directory structures | File System |
| Migration from S3 | S3 |

## Features

### Common Features

- ✅ Full TypeScript support with type definitions
- ✅ Browser and Node.js compatible
- ✅ Automatic transaction signing via polkadot-api
- ✅ Dev account support (//Alice, //Bob, etc.)

### File System SDK

- ✅ Drive creation and management
- ✅ Directory operations (create, list)
- ✅ File upload and download
- ✅ Content-addressed storage (CIDs)

### S3 SDK

- ✅ Bucket operations (create, delete, list)
- ✅ Object operations (put, get, delete, copy)
- ✅ Object metadata and user metadata
- ✅ S3-compatible naming rules
- ✅ ETag support

## API Documentation

- [File System SDK README](./file-system/README.md)
- [S3 SDK README](./s3/README.md)

## Examples

### File System Example

```bash
cd sdk/typescript/file-system
npm run example
```

### S3 Example

```bash
cd sdk/typescript/s3
npm run example
```

## Testing

```bash
# File System SDK
cd sdk/typescript/file-system
npm test

# S3 SDK
cd sdk/typescript/s3
npm test
```

## Building

```bash
# File System SDK
cd sdk/typescript/file-system
npm run build

# S3 SDK
cd sdk/typescript/s3
npm run build
```

## License

Apache-2.0
