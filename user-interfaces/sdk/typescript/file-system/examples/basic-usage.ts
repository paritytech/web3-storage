/**
 * File System SDK - Basic Usage Example
 *
 * This example demonstrates:
 * 1. Connecting to the blockchain and provider
 * 2. Creating a drive
 * 3. Uploading files
 * 4. Downloading and verifying content
 *
 * Prerequisites:
 *   - Parachain running at ws://127.0.0.1:2222
 *   - Provider running at http://127.0.0.1:3333
 *   - npm install && npm run papi:generate
 *
 * Usage:
 *   npx tsx examples/basic-usage.ts
 */

import { FileSystemClient } from "../src/index.js";

const CHAIN_WS = process.env.CHAIN_WS || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.env.PROVIDER_URL || "http://127.0.0.1:3333";

async function main() {
  console.log("=== File System SDK - Basic Usage ===\n");
  console.log(`Chain: ${CHAIN_WS}`);
  console.log(`Provider: ${PROVIDER_URL}\n`);

  // Create client
  const client = new FileSystemClient({
    chainWs: CHAIN_WS,
    providerUrl: PROVIDER_URL,
  });

  try {
    // Step 1: Connect
    console.log("Step 1: Connecting...");
    await client.connect();
    await client.setSigner("//Alice");
    console.log(`  Connected as: ${client.getAddress()}\n`);

    // Step 2: Create a drive
    console.log("Step 2: Creating drive...");
    const driveId = await client.createDrive({
      name: "My TypeScript Drive",
      capacity: 1_000_000_000n, // 1 GB
      duration: 500, // 500 blocks
      maxPayment: 1_000_000_000_000_000n, // 1000 tokens
      minProviders: 1,
    });
    console.log(`  Drive created: ID = ${driveId}\n`);

    // Step 3: Get drive info
    console.log("Step 3: Getting drive info...");
    const drive = await client.getDrive(driveId);
    console.log(`  Name: ${drive?.name}`);
    console.log(`  Bucket ID: ${drive?.bucketId}`);
    console.log(`  Owner: ${drive?.owner}\n`);

    // Step 4: Upload a file
    console.log("Step 4: Uploading file...");
    const content = new TextEncoder().encode("Hello from TypeScript SDK!");
    const uploadResult = await client.uploadFile(
      driveId,
      "/hello.txt",
      content
    );
    console.log(`  Uploaded: CID = ${uploadResult.cid}`);
    console.log(`  Size: ${uploadResult.size} bytes\n`);

    // Step 5: Download and verify
    console.log("Step 5: Downloading and verifying...");
    const bucketId = await client.getBucketId(driveId);
    const downloaded = await client.downloadByCid(bucketId, uploadResult.cid);
    const downloadedText = new TextDecoder().decode(downloaded);
    console.log(`  Downloaded: "${downloadedText}"`);

    const matches = downloadedText === "Hello from TypeScript SDK!";
    console.log(`  Verified: ${matches ? "OK" : "MISMATCH"}\n`);

    // Step 6: List drives
    console.log("Step 6: Listing all drives...");
    const drives = await client.listDrives();
    console.log(`  Found ${drives.length} drive(s):`);
    for (const d of drives) {
      console.log(`    - ID ${d.driveId}: ${d.name || "(unnamed)"}`);
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
