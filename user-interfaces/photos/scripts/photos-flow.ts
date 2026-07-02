// SPDX-License-Identifier: Apache-2.0

// Headless M1+M2+M3 flow:
//   M1 — deploy Photos → negotiate terms → createLibrary → assert.
//   M2 — mkdir album → PUT photo + thumbnail → track every op in a client-side
//        LocalIndex → setRoot from the index → assert the index root equals both
//        the on-chain anchor and the provider's index_root, then spot-check a
//        sample of files against the provider; plus a tamper check.
//   M3 — edit the photo copy-on-write: re-PUT edited bytes to the *same* path
//        (a new content-addressed blob; the pre-edit blob lingers) → update the
//        index + setRoot → download back and byte-compare → assert the anchor
//        moved, the entry set is unchanged (replace, not add), and library/
//        ownership/Writer grant survive the edit.
//
// Usage: tsx scripts/photos-flow.ts [chain_ws] [provider_url] [provider_seed] [client_seed]
//   defaults: ws://127.0.0.1:2222  http://127.0.0.1:3333  //Alice  //Bob
//
// Preconditions: a running chain + provider node, with the provider already
// registered and `accepting_primary` (e.g. after `just demo`), and a built
// artifact (`build:contract`).

import assert from "node:assert";

import {
  computeDataRoot,
  connect,
  isSameAddress,
  makeSigner,
  metadataMerkleRoot,
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
  h160ToSubstrate,
  substrateToH160,
} from "@web3-storage/sdk/revive";
import { FileSystemClient } from "@web3-storage/sdk/fs";
import { negotiatePrecompileTerms } from "./lib/contract.js";
import { LocalIndex } from "./lib/local-index.js";
import { anchorRoot, loadArtifact, readLibraryOf } from "./lib/photos.js";

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
// Blocks to wait for the best-block view to catch up to a just-included setRoot.
const ANCHOR_RETRY_BLOCKS = 5;
// Files downloaded per anchor check to catch silent data loss — the index
// already proves the root, so we sample rather than re-download the whole drive.
const SPOT_CHECK_SAMPLES = 3;

// M2 album layout: a photo under an album, its thumbnail under a parallel
// `.thumbs/` subtree (DESIGN.md "Albums, blobs & the root anchor").
const ALBUM = "/Beach";
const PHOTO = `${ALBUM}/photo.jpg`;
const THUMB = `/.thumbs${ALBUM}/photo.jpg`;
// Directories the client creates — the source of truth for the root's dir leaves.
const ALBUM_DIRS = [ALBUM, "/.thumbs", `/.thumbs${ALBUM}`];

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
  console.log("=== Photos M1+M2+M3 flow ===");
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

    // /fs ops go through the SDK's FileSystemClient: explicit providerUrl skips
    // on-chain resolution, and best-block reads/submits match this flow's
    // in-block semantics. Requests are signed when the signer has a keypair —
    // harmless against the auth-disabled dev provider, and the path the UI uses.
    const fs = new FileSystemClient({
      api,
      signer: user,
      providerUrl,
      readOpts: { at: "best" },
      submitMode: "best",
    });

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

    // Client-side source of truth for the anchored root, updated as we mkdir/upload/edit.
    const index = new LocalIndex();

    // Verify the anchor: assert local root == on-chain rootCid == provider index_root
    // (retrying while the chain lags the just-included setRoot, per `isStale`), then
    // spot-check sampled file content — the trustless catch for silent data loss.
    const verifyAnchor = async ({ isStale }: { isStale: (rootCid: string) => boolean }) => {
      const localRoot = index.root().toLowerCase();
      let anchored = await readLibraryOf(api, deployed.addressBytes, substrateToH160(user.publicKey), user.address, abi);
      for (let i = 0; i < ANCHOR_RETRY_BLOCKS && isStale(anchored.rootCid.toLowerCase()); i++) {
        await waitForNextBlock(papi);
        anchored = await readLibraryOf(api, deployed.addressBytes, substrateToH160(user.publicKey), user.address, abi);
      }
      const idx = await fs.getIndexRoot(bucketId);
      console.log(`  local root=${localRoot}`);
      console.log(`  on-chain  =${anchored.rootCid.toLowerCase()}`);
      console.log(`  index_root=${idx.indexRoot.toLowerCase()}`);
      assert.strictEqual(localRoot, anchored.rootCid.toLowerCase(), "local root != on-chain rootCid");
      assert.strictEqual(localRoot, idx.indexRoot.toLowerCase(), "local root != provider index_root");

      // Sample files deterministically (evenly spaced, so CI is reproducible) and
      // confirm the provider serves back the bytes we anchored.
      const files = index.filePaths().sort();
      const sampleCount = Math.min(SPOT_CHECK_SAMPLES, files.length);
      const sampled = Array.from({ length: sampleCount }, (_, i) => files[Math.floor((i * files.length) / sampleCount)]);
      for (const path of sampled) {
        const bytes = await fs.downloadFile(bucketId, path);
        const served = toHex(computeDataRoot(bytes)).toLowerCase();
        const expected = toHex(index.get(path)!.dataRoot).toLowerCase();
        assert.strictEqual(served, expected, `provider-served content for ${path} != the client's anchored data_root`);
      }
      console.log(`  spot-checked ${sampled.length}/${files.length} files ✓`);
      return { anchored, idx };
    };

    // ── M2: albums + blobs + thumbnails + client-computed root anchor ──
    console.log("\n=== M2: albums + blobs + root anchor ===");

    // [M2 1/5] Create the album and its parallel thumbnail subtree.
    console.log("\n[M2 1/5] mkdir album + thumbnail subtree…");
    for (const dir of ALBUM_DIRS) {
      await fs.createDirectory(bucketId, dir);
      index.setDir(dir);
    }

    // [M2 2/5] Upload a multi-MB photo (spans several 256 KiB chunks) + a small
    // placeholder thumbnail (real canvas downscaling lands in M6).
    console.log("\n[M2 2/5] PUT photo (multi-MB) + placeholder thumbnail…");
    const photoBytes = makeBytes(2 * 1024 * 1024 + 12_345, 0xc0ffee);
    const thumbBytes = makeBytes(4_096, 0xbeef);
    const photoPut = await fs.uploadFile(bucketId, PHOTO, photoBytes, { contentType: "image/jpeg" });
    const thumbPut = await fs.uploadFile(bucketId, THUMB, thumbBytes, { contentType: "image/jpeg" });
    if (!photoPut.dataRoot || !thumbPut.dataRoot) throw new Error("provider did not return a data_root for an upload");
    console.log(`  photo: data_root=${photoPut.dataRoot} size=${photoPut.size}`);
    console.log(`  thumb: data_root=${thumbPut.dataRoot} size=${thumbPut.size}`);

    // Compute each data_root locally and cross-check against the provider's echo
    // (proves our chunk-tree port matches the provider's), then record in the index.
    const photoDataRoot = computeDataRoot(photoBytes);
    const thumbDataRoot = computeDataRoot(thumbBytes);
    assert.strictEqual(toHex(photoDataRoot).toLowerCase(), photoPut.dataRoot.toLowerCase(), "local photo data_root != provider data_root");
    assert.strictEqual(toHex(thumbDataRoot).toLowerCase(), thumbPut.dataRoot.toLowerCase(), "local thumb data_root != provider data_root");
    index.setFile(PHOTO, photoDataRoot, BigInt(photoBytes.length));
    index.setFile(THUMB, thumbDataRoot, BigInt(thumbBytes.length));

    // [M2 3/5] Anchor the index root. The recursive listing only asserts the
    // provider's structure matches the client's — never as the source of the root,
    // which would just prove the provider agrees with itself.
    console.log("\n[M2 3/5] Assert provider structure + anchor index root → setRoot…");
    const listing = await fs.listDirectory(bucketId, "/", { recursive: true });
    const providerFilePaths = listing.filter((e) => e.entryType === "file").map((e) => e.path).sort();
    const providerDirPaths = listing.filter((e) => e.entryType !== "file").map((e) => e.path).sort();
    assert.deepStrictEqual(providerFilePaths, [PHOTO, THUMB].sort(), "provider's file set != the client's uploaded set (hidden/extra/renamed files)");
    assert.deepStrictEqual(providerDirPaths, [...ALBUM_DIRS].sort(), "provider's directory set != the client's created set (hidden/extra/renamed dirs)");
    const localRoot = index.root();
    console.log(`  entries=${index.entries().length} localRoot=${localRoot}`);
    await anchorRoot(api, user, deployed.addressBytes, localRoot, abi);

    // [M2 4/5] Verify the anchor and spot-check served content.
    console.log("\n[M2 4/5] Assert root == anchor == index_root + spot-check content…");
    // First anchor: the on-chain rootCid is still zero until setRoot lands.
    const { anchored } = await verifyAnchor({
      isStale: (rootCid) => /^0x0+$/.test(rootCid),
    });

    // [M2 5/5] Tamper check: a mutated entry set must NOT match the anchor.
    console.log("\n[M2 5/5] Tamper check (mutated entry set must mismatch)…");
    const tampered = index.entries();
    tampered[0] = { ...tampered[0], size: tampered[0].size + 1n };
    const tamperedRoot = (toHex(metadataMerkleRoot(tampered)) as `0x${string}`).toLowerCase();
    assert.notStrictEqual(tamperedRoot, anchored.rootCid.toLowerCase(), "tampered root unexpectedly matched the anchor");
    console.log("  tampered root differs from anchor ✓");

    // ── M3: edit a photo (copy-on-write, same path) ──
    console.log("\n=== M3: edit (copy-on-write) ===");

    // Pre-edit snapshot to prove the edit's effect (and its limits).
    const rootBeforeEdit = anchored.rootCid.toLowerCase();
    const entriesBeforeEdit = index.entries().length;
    const photoRootBeforeEdit = toHex(photoDataRoot).toLowerCase();

    // [M3 1/4] Edit in place: re-PUT *different* bytes to the SAME path. Content-
    // addressed copy-on-write writes a new blob and repoints the path; the old
    // blob lingers (no GC). Real canvas crop/rotate is M6 — a fresh seed stands in.
    console.log("\n[M3 1/4] Edit photo in place (re-PUT same path)…");
    const editedBytes = makeBytes(2 * 1024 * 1024 + 6_789, 0xed17ed);
    const editPut = await fs.uploadFile(bucketId, PHOTO, editedBytes, { contentType: "image/jpeg" });
    if (!editPut.dataRoot) throw new Error("provider did not return a data_root for the edited upload");
    const editedDataRoot = computeDataRoot(editedBytes);
    console.log(`  edited photo: data_root=${editPut.dataRoot} size=${editPut.size}`);
    assert.strictEqual(toHex(editedDataRoot).toLowerCase(), editPut.dataRoot.toLowerCase(), "local edited data_root != provider data_root");
    assert.notStrictEqual(editPut.dataRoot.toLowerCase(), photoRootBeforeEdit, "edited data_root unchanged — the COW write did not produce a new blob");
    // Same path = replace, so the entry count is unchanged and only this leaf moves.
    index.setFile(PHOTO, editedDataRoot, BigInt(editedBytes.length));

    // [M3 2/4] Recompute the metadata root over the post-edit index → re-anchor.
    console.log("\n[M3 2/4] Recompute metadata root → setRoot…");
    assert.strictEqual(index.entries().length, entriesBeforeEdit, "entry count changed after a same-path edit — expected replace, not add");
    const editedRoot = index.root();
    console.log(`  editedRoot=${editedRoot}`);
    assert.notStrictEqual(editedRoot.toLowerCase(), rootBeforeEdit, "metadata root unchanged after edit — the edit was not reflected in the tree");
    await anchorRoot(api, user, deployed.addressBytes, editedRoot, abi);

    // [M3 3/4] Re-verify the new anchor and spot-check.
    console.log("\n[M3 3/4] Re-read anchor + assert index root + spot-check…");
    // Post-edit: wait while the on-chain rootCid still reads the pre-edit value.
    const { anchored: editAnchored } = await verifyAnchor({
      isStale: (rootCid) => rootCid === rootBeforeEdit,
    });
    assert.notStrictEqual(editAnchored.rootCid.toLowerCase(), rootBeforeEdit, "on-chain rootCid did not move after the edit");

    // [M3 4/4] Download the same path back: the bytes must be the edited blob,
    // proving the path was repointed (not the original, lingering blob).
    console.log("\n[M3 4/4] Download same path + byte-compare against edited bytes…");
    const downloaded = await fs.downloadFile(bucketId, PHOTO);
    assert.deepStrictEqual(downloaded, editedBytes, "downloaded photo bytes != edited bytes — path was not repointed to the edited blob");
    console.log(`  downloaded ${downloaded.length} bytes — matches edited photo ✓`);

    // Invariants that must survive an edit: the library still exists with the
    // same driveId, the drive is still owned by the contract account, and the
    // user still holds the Writer grant on the bucket. The edit touches data
    // only — never ownership or ACLs.
    assert.ok(editAnchored.exists, "libraryOf.exists became false after edit");
    assert.strictEqual(editAnchored.driveId, driveId, "libraryOf.driveId changed after edit");
    const driveAfterEdit: any = await api.query.DriveRegistry.Drives.getValue(driveId, READ_OPTS);
    assert.ok(driveAfterEdit, "drive vanished from DriveRegistry after edit");
    assert.ok(isSameAddress(driveAfterEdit.owner, contractAccount.address), "drive owner is no longer the contract account after edit");
    const bucketAfterEdit: any = await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS);
    assert.ok(bucketAfterEdit?.members?.some((m: any) => isSameAddress(m.account, user.address)), "user lost the Writer grant after edit");

    // End state — the contract this flow deployed (an ephemeral instance,
    // distinct from `just photos deploy`) now holds the post-edit anchored root.
    console.log(`\n📌 end state — contract ${deployed.address}  libraryOf(user).rootCid = ${editAnchored.rootCid}`);
    assert.ok(!/^0x0+$/.test(editAnchored.rootCid), "end-state rootCid is still zero — setRoot did not persist");

    console.log("\n✅ Photos M1+M2+M3 flow completed — album + photo round-tripped, edited copy-on-write at the same path, client-computed root re-anchored, and verified against the on-chain anchor and index_root throughout.");
  } finally {
    papi.destroy();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
