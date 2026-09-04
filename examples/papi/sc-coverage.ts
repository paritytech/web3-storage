// SPDX-License-Identifier: Apache-2.0

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
  connect,
  ensureProviderRegistered,
  fetchChallengeProof,
  fetchCheckpointSignature,
  hexToBytes,
  makeSigner,
  READ_OPTS,
  respondToChallenge,
  sameAddress,
  submitClientCheckpoint,
  toHex,
  uploadChunk,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
  type ChainSigner,
  type ParachainApi,
} from "@web3-storage/sdk";
import {
  callContract,
  encodeCall,
  ensureAccountMapped,
} from "@web3-storage/sdk/revive";
import { negotiatePrecompileTerms, SolRole, SolVisibility } from "./sc-support.js";
import {
  ensureSoleAcceptingProvider,
  parseProviderClientArgs,
} from "./support.js";

const { chainWs, providerUrl, providerSeed, clientSeed } = parseProviderClientArgs();

const HERE = dirname(fileURLToPath(import.meta.url));
const CONTRACT_JSON = resolve(HERE, "../contracts/build/combined.json");

const WEB3_STORAGE_ADDR = hexToBytes("0x0000000000000000000000000000000009010000");
const DRIVE_REGISTRY_ADDR = hexToBytes("0x0000000000000000000000000000000009020000");

/** Send raw calldata to a precompile address as a signed substrate tx. */
async function callPrecompile(api: ParachainApi, signer: ChainSigner, addr: Uint8Array | string, abi: any, fnName: string, args: unknown[], opts = {}) {
  const data = encodeCall(abi, fnName, args);
  return callContract(api, signer, addr, data, opts);
}

/** Assert an event of the named pallet was emitted in this tx. */
function assertEvent(events: any[], type: string, valueType: string, label: string) {
  const ev = events.find(
    (e: any) => e.type === type && e.value?.type === valueType
  );
  if (!ev) {
    const seen = events
      .map((e: any) => `${e.type}::${e.value?.type ?? "?"}`)
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
    // registered by earlier demos in the CI matrix) so only the provider we
    // negotiate with holds agreements during this run.
    console.log("\n[setup] provider + account mapping…");
    const PRICE_PER_BYTE = 1n;
    await ensureProviderRegistered(api, provider, providerUrl, {
      pricePerByte: PRICE_PER_BYTE,
      maxDuration: 100_000,
    });
    await ensureSoleAcceptingProvider(api, provider);
    await ensureAccountMapped(api, provider);
    await ensureAccountMapped(api, client);
    await ensureAccountMapped(api, member);
    const providerBytes32 = provider.publicKey;
    const memberBytes32 = member.publicKey;

    /** Negotiate terms for a direct precompile call signed by `owner`. */
    const negotiateAbiTerms = (
      owner: ChainSigner,
      req: { maxBytes: bigint; duration: number; pricePerByte: bigint }
    ) => negotiatePrecompileTerms(providerUrl, owner, req);

    // ====================================================================
    // Storage-provider precompile (0x…09010000)
    // ====================================================================

    // 1. establishStorageAgreement ----------------------------------------
    // Negotiated terms create the bucket + primary agreement atomically, so
    // the agreement is already active for the top-up/extend/end steps below.
    console.log("\n[1] IWeb3Storage.establishStorageAgreement(provider, terms[2KiB×100], sig)");
    const maxBytesA = 2048n;
    const durationA = 100;
    const maxPaymentA = maxBytesA * BigInt(durationA) * 10n; // generous
    const signedA = await negotiateAbiTerms(client, {
      maxBytes: maxBytesA,
      duration: durationA,
      pricePerByte: PRICE_PER_BYTE,
    });
    let nextBucketBefore = await api.query.StorageProvider.NextBucketId.getValue();
    let r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "establishStorageAgreement", [
      toHex(providerBytes32),
      signedA.terms,
      signedA.signature,
      SolVisibility.Private, // wrapper-default tier
    ]);
    const created = assertEvent(r.events, "StorageProvider", "BucketCreated", "establishStorageAgreement");
    assertEvent(r.events, "StorageProvider", "StorageAgreementEstablished", "establishStorageAgreement");
    const bucketA = created.bucket_id;
    assert.strictEqual(bucketA, nextBucketBefore, "BucketCreated.bucket_id == pre-call NextBucketId");
    console.log("  bucketA =", bucketA.toString());

    // 2. setMember --------------------------------------------------------
    console.log("\n[2] IWeb3Storage.setMember(bucketA, Charlie, Writer)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "setMember", [
      bucketA,
      toHex(memberBytes32),
      SolRole.Writer,
    ]);
    assertEvent(r.events, "StorageProvider", "MemberSet", "setMember");
    const bucket = (await api.query.StorageProvider.Buckets.getValue(
      bucketA,
      READ_OPTS
    ))!;
    assert.ok(
      bucket.members.some((m: { account: string }) => sameAddress(m.account, member.address)),
      "Charlie should be in bucket members after setMember"
    );

    // 3. removeMember -----------------------------------------------------
    console.log("\n[3] IWeb3Storage.removeMember(bucketA, Charlie)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "removeMember", [
      bucketA,
      toHex(memberBytes32),
    ]);
    assertEvent(r.events, "StorageProvider", "MemberRemoved", "removeMember");

    // 4. topUpAgreement ---------------------------------------------------
    console.log("\n[4] IWeb3Storage.topUpAgreement(bucketA, provider, +1024 bytes, …)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "topUpAgreement", [
      bucketA,
      toHex(providerBytes32),
      1024n, // additional bytes
      maxPaymentA, // max payment
    ]);
    assertEvent(r.events, "StorageProvider", "AgreementToppedUp", "topUpAgreement");

    // 5. extendAgreement --------------------------------------------------
    console.log("\n[5] IWeb3Storage.extendAgreement(bucketA, provider, +50 blocks, …)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "extendAgreement", [
      bucketA,
      toHex(providerBytes32),
      50, // additional duration
      maxPaymentA,
    ]);
    assertEvent(r.events, "StorageProvider", "AgreementExtended", "extendAgreement");

    // 6. endAgreementPay --------------------------------------------------
    console.log("\n[6] IWeb3Storage.endAgreementPay(bucketA, provider)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "endAgreementPay", [
      bucketA,
      toHex(providerBytes32),
    ]);
    assertEvent(r.events, "StorageProvider", "AgreementEnded", "endAgreementPay");

    // 7. establishStorageAgreement (large — for endAgreementBurn) --------
    // Burn-percent transfers send to the treasury account; the transfer uses
    // `KeepAlive`, so the burned amount must be ≥ ExistentialDeposit (1
    // MILLIUNIT = 1e9 atomic). 10% of `1MiB × 100k blocks × 1` ≈ 1e10 atomic,
    // comfortably above ED.
    console.log("\n[7] IWeb3Storage.establishStorageAgreement(provider, terms[1MiB×100k], sig)  [burn-sized]");
    const signedB = await negotiateAbiTerms(client, {
      maxBytes: 1n << 20n,
      duration: 100_000,
      pricePerByte: PRICE_PER_BYTE,
    });
    // sometimes the rpc returns old data, and the tests run sequentially, so
    // bump the expected NextBucketId by hand for the post-call assertion.
    nextBucketBefore += 1n;
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "establishStorageAgreement", [
      toHex(providerBytes32),
      signedB.terms,
      signedB.signature,
      SolVisibility.Private,
    ]);
    const createdB = assertEvent(r.events, "StorageProvider", "BucketCreated", "establishStorageAgreement");
    const bucketB = createdB.bucket_id;
    assert.strictEqual(bucketB, nextBucketBefore);
    assertEvent(r.events, "StorageProvider", "StorageAgreementEstablished", "establishStorageAgreement");
    console.log("  bucketB =", bucketB.toString());

    // 8. endAgreementBurn (early-terminate bucketB) -----------------------
    console.log("\n[8] IWeb3Storage.endAgreementBurn(bucketB, provider, burn=10%)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "endAgreementBurn", [
      bucketB,
      toHex(providerBytes32),
      10,
    ]);
    assertEvent(r.events, "StorageProvider", "AgreementEnded", "endAgreementBurn");

    // 9. challengeCheckpoint + freezeBucket on a fresh small bucket -------
    // Upload + checkpoint give us both a snapshot to freeze and a leaf to
    // challenge. The agreement is left open and is not ended; settlement
    // happens through chain-driven expiry, not this test.
    console.log("\n[9] IWeb3Storage.establishStorageAgreement(provider, terms[2KiB×100], sig)  [freeze/challenge target]");
    const signedC = await negotiateAbiTerms(client, {
      maxBytes: 2048n,
      duration: 100,
      pricePerByte: PRICE_PER_BYTE,
    });
    // sometimes the rpc returns old data, and the tests run sequentially, so
    // bump the expected NextBucketId by hand for the post-call assertion.
    nextBucketBefore += 1n;
    // Finalize: the upload below reads bucketC membership from the provider's
    // finalized view, so an in-block establish would race it.
    r = await callPrecompile(
      api,
      client,
      WEB3_STORAGE_ADDR,
      iWeb3,
      "establishStorageAgreement",
      [toHex(providerBytes32), signedC.terms, signedC.signature, SolVisibility.Private],
      { finalized: true },
    );
    const createdC = assertEvent(r.events, "StorageProvider", "BucketCreated", "establishStorageAgreement");
    const bucketC = createdC.bucket_id;
    assert.strictEqual(bucketC, nextBucketBefore);
    console.log("  bucketC =", bucketC.toString());

    console.log("    preconditions: uploadChunk + submitClientCheckpoint");
    const upload = await uploadChunk(providerUrl, bucketC, "coverage-test", client);
    const ck = await fetchCheckpointSignature(providerUrl, bucketC);
    await submitClientCheckpoint(api, client, provider, bucketC, ck);

    console.log("\n[9a] IWeb3Storage.challengeCheckpoint(bucketC, provider, leafIdx, chunkIdx=0)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "challengeCheckpoint", [
      bucketC,
      toHex(providerBytes32),
      BigInt(upload.commit.leaf_indices[0]),
      0n,
    ]);
    const challenge = assertEvent(r.events, "StorageProvider", "ChallengeCreated", "challengeCheckpoint");

    console.log("    [substrate] respondToChallenge");
    const proof = await fetchChallengeProof(api, providerUrl, challenge.challenge_id);
    await respondToChallenge(api, provider, challenge.challenge_id, proof);

    console.log("\n[10] IWeb3Storage.freezeBucket(bucketC)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "freezeBucket", [bucketC]);
    assertEvent(r.events, "StorageProvider", "BucketFrozen", "freezeBucket");

    // 10a. setBucketVisibility -------------------------------------------
    console.log("\n[10a] IWeb3Storage.setBucketVisibility(bucketC, Public)");
    r = await callPrecompile(api, client, WEB3_STORAGE_ADDR, iWeb3, "setBucketVisibility", [
      bucketC,
      SolVisibility.Public,
    ]);
    assertEvent(r.events, "StorageProvider", "BucketVisibilityChanged", "setBucketVisibility");

    // ====================================================================
    // Drive-registry precompile (0x…09020000)
    // ====================================================================

    // 11. createDrive -----------------------------------------------------
    console.log("\n[11] IDriveRegistry.createDrive(\"cov\", provider, terms[1MiB×50], sig)");
    const signedD = await negotiateAbiTerms(client, {
      maxBytes: 1n << 20n,
      duration: 50,
      pricePerByte: PRICE_PER_BYTE,
    });
    const nextDriveBefore = await api.query.DriveRegistry.NextDriveId.getValue();
    r = await callPrecompile(api, client, DRIVE_REGISTRY_ADDR, iDrive, "createDrive", [
      "cov",
      toHex(providerBytes32),
      signedD.terms,
      signedD.signature,
      SolVisibility.Private,
    ]);
    const driveEvt = assertEvent(r.events, "DriveRegistry", "DriveCreated", "createDrive");
    const driveId = driveEvt.drive_id;
    assert.strictEqual(driveId, nextDriveBefore);
    console.log("  driveId =", driveId.toString());

    // 12. shareDrive ------------------------------------------------------
    console.log("\n[12] IDriveRegistry.shareDrive(driveId, Charlie, Reader)");
    r = await callPrecompile(api, client, DRIVE_REGISTRY_ADDR, iDrive, "shareDrive", [
      driveId,
      toHex(memberBytes32),
      SolRole.Reader,
    ]);
    assertEvent(r.events, "DriveRegistry", "DriveShared", "shareDrive");

    // 13. unshareDrive ----------------------------------------------------
    console.log("\n[13] IDriveRegistry.unshareDrive(driveId, Charlie)");
    r = await callPrecompile(api, client, DRIVE_REGISTRY_ADDR, iDrive, "unshareDrive", [
      driveId,
      toHex(memberBytes32),
    ]);
    assertEvent(r.events, "DriveRegistry", "DriveUnshared", "unshareDrive");

    // 14. deleteDrive -----------------------------------------------------
    console.log("\n[14] IDriveRegistry.deleteDrive(driveId)");
    r = await callPrecompile(api, client, DRIVE_REGISTRY_ADDR, iDrive, "deleteDrive", [
      driveId,
    ]);
    assertEvent(r.events, "DriveRegistry", "DriveDeleted", "deleteDrive");

    console.log("\n✅ All 13 selectors exercised, every expected event observed");
  } finally {
    papi.destroy();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
