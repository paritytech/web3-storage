// SPDX-License-Identifier: Apache-2.0

/**
 * File System SDK - Basic Usage Example
 *
 * This example demonstrates:
 * 1. Connecting to the blockchain and provider
 * 2. Creating a drive
 * 3. Creating directories
 * 4. Uploading files
 * 5. Downloading files by path
 * 6. Listing directory contents
 * 7. Deleting files
 * 8. Checking drive index root
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
      capacity: 1_073_741_824n, // 1 GB (2^30 bytes)
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

    // Step 4: Create directories
    console.log("Step 4: Creating directories...");
    await client.createDirectory(driveId, "/documents");
    console.log("  Created /documents");
    await client.createDirectory(driveId, "/documents/notes");
    console.log("  Created /documents/notes\n");

    // Step 5: Upload files
    console.log("Step 5: Uploading files...");
    const content1 = new TextEncoder().encode("Hello from TypeScript SDK!");
    const result1 = await client.uploadFile(driveId, "/documents/hello.txt", content1);
    console.log(`  Uploaded /documents/hello.txt (CID: ${result1.cid}, ${result1.size} bytes)`);

    const content2 = new TextEncoder().encode(
      JSON.stringify({ message: "Hello JSON!", timestamp: Date.now() })
    );
    const result2 = await client.uploadFile(driveId, "/documents/notes/data.json", content2, {
      contentType: "application/json",
    });
    console.log(`  Uploaded /documents/notes/data.json (${result2.size} bytes)\n`);

    // Step 6: Download and verify
    console.log("Step 6: Downloading and verifying...");
    const downloaded = await client.downloadFile(driveId, "/documents/hello.txt");
    const downloadedText = new TextDecoder().decode(downloaded.data);
    console.log(`  Downloaded: "${downloadedText}"`);
    console.log(`  Content-Type: ${downloaded.contentType}`);
    console.log(`  Size: ${downloaded.size} bytes`);
    const matches = downloadedText === "Hello from TypeScript SDK!";
    console.log(`  Verified: ${matches ? "OK" : "MISMATCH"}\n`);

    // Step 7: List directory
    console.log("Step 7: Listing /documents...");
    const listing = await client.listDirectory(driveId, "/documents");
    console.log(`  Path: ${listing.path}`);
    console.log(`  Files: ${listing.fileCount}, Dirs: ${listing.dirCount}, Total: ${listing.totalSize} bytes`);
    for (const entry of listing.entries) {
      const type = entry.entry_type === "dir" ? "[DIR]" : "[FILE]";
      console.log(`    ${type} ${entry.name} (${entry.size} bytes)`);
    }
    console.log();

    // Step 8: Check index root
    console.log("Step 8: Getting index root...");
    const indexRoot = await client.getIndexRoot(driveId);
    console.log(`  Merkle root: ${indexRoot.metadataMerkleRoot}`);
    console.log(`  Files: ${indexRoot.fileCount}, Dirs: ${indexRoot.dirCount}`);
    console.log(`  Total size: ${indexRoot.totalSize} bytes\n`);

    // Step 9: Delete a file
    console.log("Step 9: Deleting /documents/hello.txt...");
    const deleteResult = await client.deleteFile(driveId, "/documents/hello.txt");
    console.log(`  Deleted: ${deleteResult.deleted}\n`);

    // Step 10: List drives
    console.log("Step 10: Listing all drives...");
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
