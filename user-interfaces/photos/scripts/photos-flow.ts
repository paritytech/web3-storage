// SPDX-License-Identifier: GPL-3.0-only
//
// Headless M1+M2 flow:
//   M1 — deploy Photos → negotiate terms → createLibrary → assert.
//   M2 — mkdir album → PUT photo + thumbnail → compute the metadata Merkle root
//        client-side → setRoot → re-list, recompute from scratch, and assert it
//        equals both the on-chain anchor and the provider's index_root; plus a
//        tamper check. Mirrors `examples/papi/sc-team-drive.js`.
// The edit/COW step lands in M3.
//
// Usage: tsx scripts/photos-flow.ts [chain_ws] [provider_url] [provider_seed] [client_seed]
//   defaults: ws://127.0.0.1:2222  http://127.0.0.1:3333  //Alice  //Bob
//
// Preconditions: a running chain + provider node, with the provider already
// registered and `accepting_primary` (e.g. after `just demo`), and a built
// artifact (`build:contract`).

import assert from "node:assert";

import {
  connect,
  isSameAddress,
  makeSigner,
  READ_OPTS,
  requireOneEvent,
  toHex,
  waitForChainReady,
  waitForNextBlock,
} from "@web3-storage/sdk";
import {
  callContract,
  decodeContractEmitted,
  deployContract,
  encodeCall,
  ensureAccountMapped,
  substrateToH160,
} from "@web3-storage/sdk/revive";
import { h160ToSubstrate, negotiatePrecompileTerms } from "./lib/contract.js";
import { anchorRoot, loadArtifact, readLibraryOf } from "./lib/photos.js";
import { enumerateEntries, indexRoot, mkdir, putFile } from "./lib/fs-client.js";
import { computeDataRoot, metadataMerkleRoot } from "./lib/merkle.js";

// pnpm forwards a literal `--` into argv; drop it.
const args = process.argv.slice(2).filter((a) => a !== "--");
const chainWs = args[0] || "ws://127.0.0.1:2222";
const providerUrl = args[1] || "http://127.0.0.1:3333";
const providerSeed = args[2] || "//Alice";
const clientSeed = args[3] || "//Bob";

const UNIT = 10n ** 12n;
const MAX_BYTES = 1n << 20n; // 1 MiB quota
const DURATION = 50; // blocks
const LIBRARY_NAME = "my-photos";

// M2 album layout: a photo under an album, its thumbnail under a parallel
// `.thumbs/` subtree (DESIGN.md "Albums, blobs & the root anchor").
const ALBUM = "/Beach";
const PHOTO = `${ALBUM}/photo.jpg`;
const THUMB = `/.thumbs${ALBUM}/photo.jpg`;

/** Deterministic pseudo-random bytes (a stand-in for real photo/thumbnail data). */
function makeBytes(len: number, seed: number): Uint8Array {
  const out = new Uint8Array(len);
  let x = seed >>> 0;
  for (let i = 0; i < len; i++) {
    x = (Math.imul(x, 1664525) + 1013904223) >>> 0;
    out[i] = x & 0xff;
  }
  return out;
}

async function main() {
  console.log("=== Photos M1+M2 flow ===");
  console.log(" chain    :", chainWs);
  console.log(" provider :", providerUrl, `(${providerSeed})`);
  console.log(" user     :", clientSeed);

  const { abi, bin } = await loadArtifact();
  const { papi, api } = connect(chainWs);
  try {
    await waitForChainReady(api);
    await waitForNextBlock(papi);

    const provider = makeSigner(providerSeed);
    const user = makeSigner(clientSeed);

    // Precondition: provider registered + accepting. Read its locked price.
    const info: any = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
    if (!info) {
      throw new Error(`Provider ${providerSeed} (${provider.address}) is not registered. Run \`just demo\` once to register + accept, then retry.`);
    }
    if (!info.settings.accepting_primary) {
      throw new Error(`Provider ${providerSeed} is not accepting_primary. Enable it (updateProviderSettings) and retry.`);
    }
    const pricePerByte: bigint = info.settings.price_per_byte;
    const payment = pricePerByte * MAX_BYTES * BigInt(DURATION);
    const value = payment * 2n + UNIT; // generous buffer; unused reserve stays in the contract (v1)
    console.log(`\n[setup] price_per_byte=${pricePerByte}  payment=${payment}  value=${value}`);

    await ensureAccountMapped(api, provider);
    await ensureAccountMapped(api, user);

    // 1) Deploy. The user deploys + calls, so msg.sender keys their library.
    console.log("\n[1/3] Deploying Photos…");
    const deployed = await deployContract(api, user, bin);
    console.log("  contract:", deployed.address);

    // 2) Negotiate primary terms with the *contract's* mapped account as owner.
    console.log("\n[2/3] Negotiating terms + createLibrary{value}…");
    const contractAccount = h160ToSubstrate(deployed.addressBytes);
    const signed = await negotiatePrecompileTerms(providerUrl, contractAccount, {
      maxBytes: MAX_BYTES,
      duration: DURATION,
      pricePerByte,
    });
    const userAccount = toHex(user.publicKey); // bytes32 substrate AccountId32 → Writer grant
    const createData = encodeCall(abi, "createLibrary", [
      userAccount,
      LIBRARY_NAME,
      toHex(provider.publicKey),
      signed.terms,
      signed.signature,
    ]);
    const r = await callContract(api, user, deployed.addressBytes, createData, { value });

    // 3) Assert on events.
    const driveCreated: any = requireOneEvent(r.events, api.event.DriveRegistry.DriveCreated, "DriveRegistry.DriveCreated");
    const driveShared: any = requireOneEvent(r.events, api.event.DriveRegistry.DriveShared, "DriveRegistry.DriveShared");
    const driveId: bigint = driveCreated.drive_id;
    const bucketId: bigint = driveCreated.bucket_id;
    console.log(`\n[3/3] driveId=${driveId}  bucketId=${bucketId}  owner=${driveCreated.owner}`);

    assert.ok(isSameAddress(driveCreated.owner, contractAccount.address), "drive owner is not the contract account");
    assert.ok(isSameAddress(driveShared.member, user.address), "Writer grant member is not the user");

    const contractLogs = decodeContractEmitted(r.events, api, deployed.address, abi);
    assert.ok(contractLogs.some((l) => l.eventName === "LibraryCreated"), "LibraryCreated event not emitted");

    // Soft cross-check: the user shows up as a member on the underlying bucket.
    const bucket: any = await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS);
    const member = bucket?.members?.find((m: any) => isSameAddress(m.account, user.address));
    console.log("  bucket member role for user:", member ? (member.role?.type ?? member.role) : "(not found)");

    // Unsigned state-detection read (the path the UI uses). rootCid is still
    // zero here — it is anchored later, in the M2 section below.
    const lib = await readLibraryOf(api, deployed.addressBytes, substrateToH160(user.publicKey), user.address, abi);
    console.log("  libraryOf(user) [pre-anchor]:", lib);
    assert.ok(lib.exists, "libraryOf.exists is false");
    assert.strictEqual(lib.driveId, driveId, "libraryOf.driveId mismatch");

    // ── M2: albums + blobs + thumbnails + client-computed root anchor ──
    console.log("\n=== M2: albums + blobs + root anchor ===");

    // [M2 1/5] Create the album and its parallel thumbnail subtree.
    console.log("\n[M2 1/5] mkdir album + thumbnail subtree…");
    await mkdir(providerUrl, bucketId, ALBUM);
    await mkdir(providerUrl, bucketId, "/.thumbs");
    await mkdir(providerUrl, bucketId, `/.thumbs${ALBUM}`);

    // [M2 2/5] Upload a multi-MB photo (spans several 256 KiB chunks) + a small
    // placeholder thumbnail (real canvas downscaling lands in M6).
    console.log("\n[M2 2/5] PUT photo (multi-MB) + placeholder thumbnail…");
    const photoBytes = makeBytes(2 * 1024 * 1024 + 12_345, 0xc0ffee);
    const thumbBytes = makeBytes(4_096, 0xbeef);
    const photoPut = await putFile(providerUrl, bucketId, PHOTO, photoBytes, "image/jpeg");
    const thumbPut = await putFile(providerUrl, bucketId, THUMB, thumbBytes, "image/jpeg");
    console.log(`  photo: data_root=${photoPut.dataRoot} size=${photoPut.size}`);
    console.log(`  thumb: data_root=${thumbPut.dataRoot} size=${thumbPut.size}`);

    // Per-file cross-check: the data_root we compute locally must match the
    // provider's (proves our chunk-tree port matches the provider's).
    assert.strictEqual(toHex(computeDataRoot(photoBytes)).toLowerCase(), photoPut.dataRoot.toLowerCase(), "local photo data_root != provider data_root");
    assert.strictEqual(toHex(computeDataRoot(thumbBytes)).toLowerCase(), thumbPut.dataRoot.toLowerCase(), "local thumb data_root != provider data_root");

    // [M2 3/5] Compute the metadata Merkle root locally → anchor it on-chain.
    console.log("\n[M2 3/5] Compute metadata root locally → setRoot…");
    const entries = await enumerateEntries(providerUrl, bucketId);
    console.log(`  entries=${entries.length}`, entries.map((e) => e.path));
    assert.ok(
      entries.length > 0,
      `drive listing is empty after mkdir/PUT — the /fs writes or recursive ls did not populate the index (bucket ${bucketId})`,
    );
    const localRoot = toHex(metadataMerkleRoot(entries)) as `0x${string}`;
    console.log(`  localRoot=${localRoot}`);
    await anchorRoot(api, user, deployed.addressBytes, localRoot, abi);

    // [M2 4/5] Verify: recompute from a fresh listing (downloading every file)
    // and assert it equals the on-chain anchor and (sanity) the index_root.
    console.log("\n[M2 4/5] Re-list + recompute + assert against on-chain anchor…");
    const fresh = await enumerateEntries(providerUrl, bucketId);
    const recomputed = (toHex(metadataMerkleRoot(fresh)) as `0x${string}`).toLowerCase();
    // Read the anchor back; retry briefly in case the best-block view lags the
    // just-included setRoot (a still-zero rootCid here means the write missed).
    let anchored = await readLibraryOf(api, deployed.addressBytes, substrateToH160(user.publicKey), user.address, abi);
    for (let i = 0; i < 5 && /^0x0+$/.test(anchored.rootCid); i++) {
      await waitForNextBlock(papi);
      anchored = await readLibraryOf(api, deployed.addressBytes, substrateToH160(user.publicKey), user.address, abi);
    }
    const idx = await indexRoot(providerUrl, bucketId);
    console.log(`  recomputed=${recomputed}`);
    console.log(`  on-chain  =${anchored.rootCid.toLowerCase()}`);
    console.log(`  index_root=${idx.metadataMerkleRoot.toLowerCase()}`);
    assert.strictEqual(recomputed, anchored.rootCid.toLowerCase(), "recomputed root != on-chain rootCid");
    assert.strictEqual(recomputed, idx.metadataMerkleRoot.toLowerCase(), "recomputed root != provider index_root");

    // [M2 5/5] Tamper check: a mutated entry set must NOT match the anchor.
    console.log("\n[M2 5/5] Tamper check (mutated entry set must mismatch)…");
    const tampered = fresh.map((e) => ({ ...e }));
    tampered[0].size += 1n;
    const tamperedRoot = (toHex(metadataMerkleRoot(tampered)) as `0x${string}`).toLowerCase();
    assert.notStrictEqual(tamperedRoot, anchored.rootCid.toLowerCase(), "tampered root unexpectedly matched the anchor");
    console.log("  tampered root differs from anchor ✓");

    // End-state read-back — the contract this flow deployed (an ephemeral
    // instance, distinct from `just photos deploy`) now holds the anchored root.
    const end = await readLibraryOf(api, deployed.addressBytes, substrateToH160(user.publicKey), user.address, abi);
    console.log(`\n📌 end state — contract ${deployed.address}  libraryOf(user).rootCid = ${end.rootCid}`);
    assert.ok(!/^0x0+$/.test(end.rootCid), "end-state rootCid is still zero — setRoot did not persist");

    console.log("\n✅ Photos M1+M2 flow completed — album + photo round-tripped, client-computed root anchored and verified against the on-chain anchor and index_root.");
  } finally {
    papi.destroy();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
