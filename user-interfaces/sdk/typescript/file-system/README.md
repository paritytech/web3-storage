# Web3 Storage File System SDK (TypeScript)

TypeScript/JavaScript SDK for the Web3 Storage File System Interface.

## Installation

```bash
npm install @web3-storage/file-system-client
```

## Quick Start

```typescript
import { FileSystemClient } from "@web3-storage/file-system-client";

async function main() {
  // Create client
  const client = new FileSystemClient({
    chainWs: "ws://127.0.0.1:2222",
    providerUrl: "http://127.0.0.1:3333",
  });

  // Connect and set signer
  await client.connect();
  await client.setSigner("//Alice"); // Dev account

  // Create a drive
  const driveId = await client.createDrive({
    name: "My Drive",
    capacity: 10_000_000_000n, // 10 GB
    duration: 500, // blocks
    maxPayment: 1_000_000_000_000_000n, // 1000 tokens
  });
  console.log("Created drive:", driveId);

  // Upload a file
  const data = new TextEncoder().encode("Hello, Web3 Storage!");
  const result = await client.uploadFile(driveId, "/hello.txt", data);
  console.log("Uploaded with CID:", result.cid);

  // Download by CID
  const bucketId = await client.getBucketId(driveId);
  const downloaded = await client.downloadByCid(bucketId, result.cid);
  console.log("Downloaded:", new TextDecoder().decode(downloaded));

  // Cleanup
  client.disconnect();
}

main();
```

## Setup (Development)

### Prerequisites

- Node.js 18+
- Running parachain (ws://127.0.0.1:2222)
- Running provider (http://127.0.0.1:3333)

### Generate Chain Descriptors

Before using the SDK, generate the chain type descriptors:

```bash
# Start the parachain first, then:
npm install
npm run papi:generate
```

### Run Example

```bash
npm run example
```

## API Reference

### FileSystemClient

#### Constructor

```typescript
new FileSystemClient(config: FileSystemConfig)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `config.chainWs` | string | Parachain WebSocket URL |
| `config.providerUrl` | string | Provider HTTP URL |

#### Methods

##### Connection

| Method | Description |
|--------|-------------|
| `connect()` | Connect to the blockchain |
| `setSigner(seed)` | Set the transaction signer |
| `getAddress()` | Get the signer's address |
| `disconnect()` | Disconnect from the blockchain |

##### Drive Operations

| Method | Description |
|--------|-------------|
| `createDrive(options)` | Create a new drive |
| `getDrive(driveId)` | Get drive information |
| `getBucketId(driveId)` | Get the Layer 0 bucket ID |
| `listDrives()` | List all drives owned by the user |
| `deleteDrive(driveId)` | Delete a drive |
| `clearDrive(driveId)` | Clear drive contents |

##### File Operations

| Method | Description |
|--------|-------------|
| `uploadFile(driveId, path, data, options?)` | Upload a file |
| `downloadByCid(bucketId, cid)` | Download content by CID |
| `createDirectory(driveId, path)` | Create a directory |
| `listDirectory(driveId, path)` | List directory contents |

### Types

#### CreateDriveOptions

```typescript
interface CreateDriveOptions {
  name?: string;           // Drive name
  capacity: bigint;        // Storage capacity in bytes
  duration: number;        // Duration in blocks
  maxPayment: bigint;      // Maximum payment (12 decimals)
  minProviders?: number;   // Minimum providers (default: 1)
  commitStrategy?: CommitStrategy;
}
```

#### DriveInfo

```typescript
interface DriveInfo {
  driveId: bigint;
  owner: string;
  name: string | null;
  bucketId: bigint;
  rootCid: string | null;
  createdAt: bigint;
  updatedAt: bigint;
}
```

#### UploadResult

```typescript
interface UploadResult {
  cid: string;    // Content hash
  size: number;   // Size in bytes
}
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `CHAIN_WS` | ws://127.0.0.1:2222 | Parachain WebSocket URL |
| `PROVIDER_URL` | http://127.0.0.1:3333 | Provider HTTP URL |

## License

Apache-2.0
