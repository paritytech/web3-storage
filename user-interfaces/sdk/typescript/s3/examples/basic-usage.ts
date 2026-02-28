/**
 * S3 SDK - Basic Usage Example
 *
 * This example demonstrates:
 * 1. Connecting to the blockchain and provider
 * 2. Creating a bucket
 * 3. Uploading objects
 * 4. Listing and downloading objects
 *
 * Prerequisites:
 *   - Parachain running at ws://127.0.0.1:2222
 *   - Provider running at http://127.0.0.1:3333
 *   - npm install && npm run papi:generate
 *
 * Usage:
 *   npx tsx examples/basic-usage.ts
 */

import { S3Client } from "../src/index.js";

const CHAIN_WS = process.env.CHAIN_WS || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.env.PROVIDER_URL || "http://127.0.0.1:3333";

async function main() {
  console.log("=== S3 SDK - Basic Usage ===\n");
  console.log(`Chain: ${CHAIN_WS}`);
  console.log(`Provider: ${PROVIDER_URL}\n`);

  // Create client
  const client = new S3Client({
    chainWs: CHAIN_WS,
    providerUrl: PROVIDER_URL,
  });

  try {
    // Step 1: Connect
    console.log("Step 1: Connecting...");
    await client.connect();
    await client.setSigner("//Alice");
    console.log(`  Connected as: ${client.getAddress()}\n`);

    // Step 2: Create a bucket
    const bucketName = `test-bucket-${Date.now()}`;
    console.log(`Step 2: Creating bucket '${bucketName}'...`);
    const bucket = await client.createBucket(bucketName);
    console.log(`  Bucket created:`);
    console.log(`    S3 Bucket ID: ${bucket.s3BucketId}`);
    console.log(`    Layer 0 Bucket ID: ${bucket.layer0BucketId}`);
    console.log(`    Owner: ${bucket.owner}\n`);

    // Step 3: Upload objects
    console.log("Step 3: Uploading objects...");

    const textContent = new TextEncoder().encode("Hello from S3 SDK!");
    const result1 = await client.putObject(bucketName, "hello.txt", textContent, {
      contentType: "text/plain",
    });
    console.log(`  Uploaded 'hello.txt':`);
    console.log(`    CID: ${result1.cid}`);
    console.log(`    ETag: ${result1.etag}`);
    console.log(`    Size: ${result1.size} bytes`);

    const jsonContent = new TextEncoder().encode(
      JSON.stringify({ message: "Hello JSON!", timestamp: Date.now() })
    );
    const result2 = await client.putObject(
      bucketName,
      "data/config.json",
      jsonContent,
      {
        contentType: "application/json",
        metadata: { "x-custom-key": "custom-value" },
      }
    );
    console.log(`  Uploaded 'data/config.json':`);
    console.log(`    CID: ${result2.cid}`);
    console.log(`    Size: ${result2.size} bytes\n`);

    // Step 4: Get object metadata
    console.log("Step 4: Getting object metadata...");
    const metadata = await client.headObject(bucketName, "hello.txt");
    console.log(`  Object 'hello.txt' metadata:`);
    console.log(`    Size: ${metadata.size}`);
    console.log(`    Content-Type: ${metadata.contentType}`);
    console.log(`    ETag: ${metadata.etag}\n`);

    // Step 5: Download and verify
    console.log("Step 5: Downloading and verifying...");
    const downloaded = await client.getObject(bucketName, "hello.txt");
    const downloadedText = new TextDecoder().decode(downloaded.data);
    console.log(`  Downloaded: "${downloadedText}"`);

    const matches = downloadedText === "Hello from S3 SDK!";
    console.log(`  Verified: ${matches ? "OK" : "MISMATCH"}\n`);

    // Step 6: List buckets
    console.log("Step 6: Listing all buckets...");
    const buckets = await client.listBuckets();
    console.log(`  Found ${buckets.length} bucket(s):`);
    for (const b of buckets) {
      console.log(`    - ${b.name} (ID: ${b.s3BucketId}, Objects: ${b.objectCount})`);
    }

    console.log("\n=== Example completed successfully! ===");
  } catch (error) {
    console.error("\nError:", error);
    process.exitCode = 1;
  } finally {
    client.disconnect();
  }
}

main();
