# Web3 Storage S3 SDK (TypeScript)

TypeScript/JavaScript SDK for the Web3 Storage S3-Compatible Interface.

## Installation

```bash
npm install @web3-storage/s3-sdk
```

## Quick Start

```typescript
import { S3Client } from "@web3-storage/s3-sdk";

async function main() {
  // Create client
  const client = new S3Client({
    chainWs: "ws://127.0.0.1:2222",
    providerUrl: "http://127.0.0.1:3333",
  });

  // Connect and set signer
  await client.connect();
  await client.setSigner("//Alice"); // Dev account

  // Create a bucket
  const bucket = await client.createBucket("my-bucket");
  console.log("Created bucket:", bucket.name);

  // Upload an object
  const data = new TextEncoder().encode("Hello, S3!");
  const result = await client.putObject("my-bucket", "hello.txt", data, {
    contentType: "text/plain",
  });
  console.log("Uploaded with CID:", result.cid);

  // Download an object
  const obj = await client.getObject("my-bucket", "hello.txt");
  console.log("Downloaded:", new TextDecoder().decode(obj.data));

  // List buckets
  const buckets = await client.listBuckets();
  for (const b of buckets) {
    console.log(`- ${b.name}: ${b.objectCount} objects`);
  }

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

### S3Client

#### Constructor

```typescript
new S3Client(config: S3Config)
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

##### Bucket Operations

| Method | Description |
|--------|-------------|
| `createBucket(name, options?)` | Create a new S3 bucket |
| `deleteBucket(name)` | Delete an empty bucket |
| `headBucket(name)` | Get bucket information |
| `listBuckets()` | List all buckets owned by the user |

##### Object Operations

| Method | Description |
|--------|-------------|
| `putObject(bucket, key, data, options?)` | Upload an object |
| `getObject(bucket, key)` | Download an object |
| `deleteObject(bucket, key)` | Delete an object |
| `headObject(bucket, key)` | Get object metadata |
| `copyObject(srcBucket, srcKey, dstBucket, dstKey)` | Copy an object |
| `listObjectsV2(bucket, params?)` | List objects in a bucket |

### Types

#### BucketInfo

```typescript
interface BucketInfo {
  s3BucketId: bigint;
  name: string;
  layer0BucketId: bigint;
  owner: string;
  createdAt: bigint;
  objectCount: bigint;
  totalSize: bigint;
}
```

#### PutObjectOptions

```typescript
interface PutObjectOptions {
  contentType?: string;
  metadata?: Record<string, string>;
}
```

#### PutObjectResponse

```typescript
interface PutObjectResponse {
  cid: string;    // Content hash
  etag: string;   // ETag (derived from CID)
  size: number;   // Size in bytes
}
```

#### ObjectMetadata

```typescript
interface ObjectMetadata {
  key: string;
  cid: string;
  size: number;
  lastModified: bigint;
  contentType: string | null;
  etag: string;
  metadata: Record<string, string>;
}
```

#### ListObjectsParams

```typescript
interface ListObjectsParams {
  prefix?: string;           // Filter by key prefix
  delimiter?: string;        // Delimiter for grouping
  maxKeys?: number;          // Max results
  continuationToken?: string; // Pagination token
}
```

## S3 Compatibility

This SDK provides S3-compatible semantics with the following operations:

| S3 Operation | SDK Method | Status |
|--------------|------------|--------|
| CreateBucket | `createBucket()` | ✅ |
| DeleteBucket | `deleteBucket()` | ✅ |
| HeadBucket | `headBucket()` | ✅ |
| ListBuckets | `listBuckets()` | ✅ |
| PutObject | `putObject()` | ✅ |
| GetObject | `getObject()` | ✅ |
| DeleteObject | `deleteObject()` | ✅ |
| HeadObject | `headObject()` | ✅ |
| CopyObject | `copyObject()` | ✅ |
| ListObjectsV2 | `listObjectsV2()` | ✅ |

### Not Yet Implemented

- Multipart uploads
- Range requests (partial downloads)
- Versioning
- ACLs and bucket policies

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `CHAIN_WS` | ws://127.0.0.1:2222 | Parachain WebSocket URL |
| `PROVIDER_URL` | http://127.0.0.1:3333 | Provider HTTP URL |

## License

[Apache-2.0](../../../../LICENSE-APACHE2)
