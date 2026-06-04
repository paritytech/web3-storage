/**
 * Precompile-coverage e2e — exercises every selector on both precompiles by
 * making direct `Revive.call(<precompile-address>, <calldata>)` substrate
 * transactions. No marketplace contract in between; just the bare precompile
 * surface and its on-chain effect.
 *
 * Each selector gets one happy-path invocation, and the script asserts the
 * pallet's storage or events were updated as expected. Preconditions
 * (bucket existence, accepted agreement, checkpoint) are chained where
 * necessary; provider-side ops (`accept_agreement`, `respond_to_challenge`,
 * `submitClientCheckpoint`) stay on the substrate side via existing
 * `api.js` helpers since the precompile only covers the client surface.
 *
 * Prerequisites:
 *   - Chain at ws://127.0.0.1:2222 with pallet_revive wired in.
 *   - Provider node at PROVIDER_URL.
 *   - `examples/contracts/build/combined.json` (run `just build-contracts`).
 *   - PAPI descriptors (run `just papi-setup`).
 */

import assert from "node:assert";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  acceptAgreement,
  fetchCheckpointSignature,
  respondToChallenge,
  submitClientCheckpoint,
  uploadChunk,
} from "./api.js";
import {
  connect,
  ensureProviderRegistered,
  ensureSoleAcceptingProvider,
  hexToBytes,
  makeSigner,
  parseProviderClientArgs,
  READ_OPTS,
  sameAddress,
  toHex,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
} from "./common.js";
import { callContract, encodeCall, ensureAccountMapped } from "./sc-api.js";

const { chainWs, providerUrl, providerSeed, clientSeed } = parseProviderClientArgs();

const HERE = dirname(fileURLToPath(import.meta.url));
const CONTRACT_JSON = resolve(HERE, "../contracts/build/combined.json");

const WEB3_STORAGE_ADDR = hexToBytes("0x0000000000000000000000000000000009010000");
const DRIVE_REGISTRY_ADDR = hexToBytes("0x0000000000000000000000000000000009020000");

const UNIT = 10n ** 12n;

/** Send raw calldata to a precompile address as a signed substrate tx. */
async function callPrecompile(api, signer, addr, abi, fnName, args, opts = {}) {
  const data = encodeCall(abi, fnName, args);
  return callContract(api, signer, addr, data, opts);
}

/** Assert an event of the named pallet was emitted in this tx. */
function assertEvent(events, type, valueType, label) {
  const ev = events.find(
    (e) => e.type === type && e.value?.type === valueType
  );
  if (!ev) {
    const seen = events
      .map((e) => `${e.type}::${e.value?.type ?? "?"}`)
      .join(", ");
    throw new Error(`expected ${type}::${valueType} after ${label}, saw: ${seen}`);
  }
  return ev.value.value;
}

async function main() {
  console.log("=== Precompile coverage e2e ===");
  console.log(" chain    :", chainWs);
  console.log(" provider :", providerUrl, `(${providerSeed})`);
  console.log(" client   :", clientSeed);

  const { papi, api } = await connect(chainWs);
  try {
    await waitForChainReady(api);
    await waitForBlockProduction(api);
    await waitForNextBlock(papi);

    const provider = makeSigner(providerSeed);
    const client = makeSigner(clientSeed); // //Bob
    const member = makeSigner("//Charlie"); // 3rd party for membership/share tests

    const combined = JSON.parse(await readFile(CONTRACT_JSON, "utf8"));
    const iWeb3 = combined.contracts["IWeb3Storage.sol:IWeb3Storage"].abi;
    const iDrive = combined.contracts["IDriveRegistry.sol:IDriveRegistry"].abi;

    // -------- Setup --------------------------------------------------------
    // Silence any other dev-key providers (Charlie/Ferdie may have been
    // registered by earlier demos in the CI matrix) so create_bucket_with_storage
    // auto-matching is deterministic and the substrate-side challenge lookups
    // at (bucket_id, provider) hit the agreement we just created.
    console.log("\n[setup] provider + account mapping…");
    await ensureProviderRegistered(api, provider, providerUrl, {
      pricePerByte: 1n,
      maxDuration: 100_000,
    });
    await ensureSoleAcceptingProvider(api, provider);
    await ensureAccountMapped(api, provider);
    await ensureAccountMapped(api, client);
    await ensureAccountMapped(api, member);
    const providerBytes32 = provider.publicKey;
    const memberBytes32 = member.publicKey;

    // ====================================================================
    // Storage-provider precompile (0x…09010000)
    // ====================================================================

    // 1. createBucket -----------------------------------------------------
    console.log("\n[1] IWeb3Storage.createBucket(1)");
    let nextBucketBefore =
      await api.query.StorageProvider.NextBucketId.getValue(READ_OPTS);
    let r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "createBucket", [1]);
    const created = assertEvent(r.events, "StorageProvider", "BucketCreated", "createBucket");
    const bucketA = created.bucket_id;
    assert.strictEqual(bucketA, nextBucketBefore, "BucketCreated.bucket_id == pre-call NextBucketId");
    console.log("  bucketA =", bucketA.toString());

    // 2. setMember --------------------------------------------------------
    console.log("\n[2] IWeb3Storage.setMember(bucketA, Charlie, Writer)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "setMember", [
      bucketA,
      toHex(memberBytes32),
      1, // Writer
    ]);
    assertEvent(r.events, "StorageProvider", "MemberSet", "setMember");
    let bucket = await api.query.StorageProvider.Buckets.getValue(
      bucketA,
      READ_OPTS
    );
    assert.ok(
      bucket.members.some((m) => sameAddress(m.account, member.address)),
      "Charlie should be in bucket members after setMember"
    );

    // 3. removeMember -----------------------------------------------------
    console.log("\n[3] IWeb3Storage.removeMember(bucketA, Charlie)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "removeMember", [
      bucketA,
      toHex(memberBytes32),
    ]);
    assertEvent(r.events, "StorageProvider", "MemberRemoved", "removeMember");

    // 4. requestPrimaryAgreement ------------------------------------------
    console.log("\n[4] IWeb3Storage.requestPrimaryAgreement(bucketA, provider, …)");
    const maxBytesA = 2048n;
    const durationA = 100;
    const maxPaymentA = maxBytesA * BigInt(durationA) * 10n; // generous
    r = await callPrecompile(
      api,
      client,
      WEB3_STORAGE_ADDR,
      iWeb3,
      "requestPrimaryAgreement",
      [bucketA, toHex(providerBytes32), maxBytesA, durationA, maxPaymentA]
    );
    assertEvent(r.events, "StorageProvider", "AgreementRequested", "requestPrimaryAgreement");

    // Provider-side accept (substrate-native).
    console.log("    [substrate] acceptAgreement");
    await acceptAgreement(api, provider, bucketA);

    // 5. topUpAgreement ---------------------------------------------------
    console.log("\n[5] IWeb3Storage.topUpAgreement(bucketA, provider, +1024 bytes, …)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "topUpAgreement", [
      bucketA,
      toHex(providerBytes32),
      1024n, // additional bytes
      maxPaymentA, // max payment
    ]);
    assertEvent(r.events, "StorageProvider", "AgreementToppedUp", "topUpAgreement");

    // 6. extendAgreement --------------------------------------------------
    console.log("\n[6] IWeb3Storage.extendAgreement(bucketA, provider, +50 blocks, …)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "extendAgreement", [
      bucketA,
      toHex(providerBytes32),
      50, // additional duration
      maxPaymentA,
    ]);
    assertEvent(r.events, "StorageProvider", "AgreementExtended", "extendAgreement");

    // 7. endAgreementPay --------------------------------------------------
    console.log("\n[7] IWeb3Storage.endAgreementPay(bucketA, provider)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "endAgreementPay", [
      bucketA,
      toHex(providerBytes32),
    ]);
    assertEvent(r.events, "StorageProvider", "AgreementEnded", "endAgreementPay");

    // 8. createBucketWithStorage (large — for endAgreementBurn) ----------
    // Burn-percent transfers send to the treasury account; the transfer uses
    // `KeepAlive`, so the burned amount must be ≥ ExistentialDeposit (1
    // MILLIUNIT = 1e9 atomic). 10% of `1MiB × 100k blocks × 1` ≈ 1e10 atomic,
    // comfortably above ED.
    console.log("\n[8] IWeb3Storage.createBucketWithStorage(1MiB, 100k blocks, maxPrice=10)  [burn-sized]");
    nextBucketBefore =
      await api.query.StorageProvider.NextBucketId.getValue(READ_OPTS);
    r = await callPrecompile(
      api,
      client,
      WEB3_STORAGE_ADDR,
      iWeb3,
      "createBucketWithStorage",
      [1n << 20n, 100_000, 10n],
      { value: 200n * UNIT } // contract balance for payment reserve
    );
    const createdB = assertEvent(r.events, "StorageProvider", "BucketCreated", "createBucketWithStorage");
    const bucketB = createdB.bucket_id;
    assert.strictEqual(bucketB, nextBucketBefore);
    assertEvent(r.events, "StorageProvider", "AgreementAccepted", "createBucketWithStorage");
    console.log("  bucketB =", bucketB.toString());

    // 9. endAgreementBurn (early-terminate bucketB) -----------------------
    console.log("\n[9] IWeb3Storage.endAgreementBurn(bucketB, provider, burn=10%)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "endAgreementBurn", [
      bucketB,
      toHex(providerBytes32),
      10,
    ]);
    assertEvent(r.events, "StorageProvider", "AgreementEnded", "endAgreementBurn");

    // 10. challengeCheckpoint + freezeBucket on a fresh small bucket ------
    // Upload + checkpoint give us both a snapshot to freeze and a leaf to
    // challenge. The agreement is left open and is not ended; settlement
    // happens through chain-driven expiry, not this test.
    console.log("\n[10] IWeb3Storage.createBucketWithStorage(2KiB, 100, maxPrice=10)  [freeze/challenge target]");
    nextBucketBefore =
      await api.query.StorageProvider.NextBucketId.getValue(READ_OPTS);
    r = await callPrecompile(
      api,
      client,
      WEB3_STORAGE_ADDR,
      iWeb3,
      "createBucketWithStorage",
      [2048n, 100, 10n],
      { value: 5n * UNIT }
    );
    const createdC = assertEvent(r.events, "StorageProvider", "BucketCreated", "createBucketWithStorage");
    const bucketC = createdC.bucket_id;
    assert.strictEqual(bucketC, nextBucketBefore);
    console.log("  bucketC =", bucketC.toString());

    console.log("    preconditions: uploadChunk + submitClientCheckpoint");
    const upload = await uploadChunk(providerUrl, bucketC, "coverage-test");
    const ck = await fetchCheckpointSignature(providerUrl, bucketC);
    await submitClientCheckpoint(api, client, provider, bucketC, ck);

    console.log("\n[10a] IWeb3Storage.challengeCheckpoint(bucketC, provider, leafIdx, chunkIdx=0)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "challengeCheckpoint", [
      bucketC,
      toHex(providerBytes32),
      BigInt(upload.commit.leaf_indices[0]),
      0n,
    ]);
    const challenge = assertEvent(r.events, "StorageProvider", "ChallengeCreated", "challengeCheckpoint");

    console.log("    [substrate] respondToChallenge");
    const proof = await import("./api.js").then((m) =>
      m.fetchChallengeProof(api, providerUrl, challenge.challenge_id)
    );
    await respondToChallenge(api, provider, challenge.challenge_id, proof);

    console.log("\n[11] IWeb3Storage.freezeBucket(bucketC)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "freezeBucket", [bucketC]);
    assertEvent(r.events, "StorageProvider", "BucketFrozen", "freezeBucket");

    // ====================================================================
    // Drive-registry precompile (0x…09020000)
    // ====================================================================

    // 12. createDrive -----------------------------------------------------
    console.log("\n[12] IDriveRegistry.createDrive(\"cov\", 1MiB, 50 blocks, 1 UNIT, default-providers)");
    const nextDriveBefore =
      await api.query.DriveRegistry.NextDriveId.getValue(READ_OPTS);
    r = await callPrecompile(api, client, DRIVE_REGISTRY_ADDR, iDrive, "createDrive", [
      "cov",
      1n << 20n, // 1 MiB
      50, // storagePeriod blocks
      UNIT, // payment
      0, // minProviders=0 → None (use runtime default)
    ]);
    const driveEvt = assertEvent(r.events, "DriveRegistry", "DriveCreated", "createDrive");
    const driveId = driveEvt.drive_id;
    assert.strictEqual(driveId, nextDriveBefore);
    console.log("  driveId =", driveId.toString());

    // 13. shareDrive ------------------------------------------------------
    console.log("\n[13] IDriveRegistry.shareDrive(driveId, Charlie, Reader)");
    r = await callPrecompile(api, client, DRIVE_REGISTRY_ADDR, iDrive, "shareDrive", [
      driveId,
      toHex(memberBytes32),
      2, // Reader
    ]);
    assertEvent(r.events, "DriveRegistry", "DriveShared", "shareDrive");

    // 14. unshareDrive ----------------------------------------------------
    console.log("\n[14] IDriveRegistry.unshareDrive(driveId, Charlie)");
    r = await callPrecompile(api, client, DRIVE_REGISTRY_ADDR, iDrive, "unshareDrive", [
      driveId,
      toHex(memberBytes32),
    ]);
    assertEvent(r.events, "DriveRegistry", "DriveUnshared", "unshareDrive");

    // 15. deleteDrive -----------------------------------------------------
    console.log("\n[15] IDriveRegistry.deleteDrive(driveId)");
    r = await callPrecompile(api, client, DRIVE_REGISTRY_ADDR, iDrive, "deleteDrive", [
      driveId,
    ]);
    assertEvent(r.events, "DriveRegistry", "DriveDeleted", "deleteDrive");

    console.log("\n✅ All 15 selectors exercised, every expected event observed");
  } finally {
    papi.destroy();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
