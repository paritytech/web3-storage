// SPDX-License-Identifier: Apache-2.0

import { Binary, Enum } from "@polkadot-api/substrate-bindings";
import { blake2b256 } from "@polkadot-labs/hdkd-helpers";
import {
  hexToBytes,
  providerFetch,
  READ_OPTS,
  requireOneEvent,
  submitTx,
  submitTxFinalized,
  toHex,
} from "./common.js";

// ============================================================================
// StorageProvider pallet
// ============================================================================

export async function registerProvider(api, provider, providerUrl, stake = 1_000_000_000_000_000n) {
  const port = new URL(providerUrl).port;
  const multiaddr = new TextEncoder().encode(`/ip4/127.0.0.1/tcp/${port}`);
  return submitTx(
    api.tx.StorageProvider.register_provider({
      multiaddr: Binary.fromBytes(multiaddr),
      public_key: Binary.fromBytes(provider.publicKey),
      stake,
    }),
    provider.signer,
    "register_provider"
  );
}

export async function updateProviderSettings(api, provider, settings) {
  return submitTx(
    api.tx.StorageProvider.update_provider_settings({ settings }),
    provider.signer,
    "update_provider_settings"
  );
}

/**
 * POST /negotiate on the provider node; returns the SignedTerms bundle the
 * owner redeems via `establish_storage_agreement`.
 */
export async function negotiateTerms(providerUrl, request) {
  const res = await fetch(`${providerUrl}/negotiate`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(request, (_k, v) =>
      typeof v === "bigint" ? v.toString() : v,
    ),
  });
  if (!res.ok) {
    throw new Error(
      `/negotiate failed: ${res.status} ${await res.text().catch(() => "")}`
    );
  }
  return res.json();
}

/**
 * GET /info on the provider node
 */
export async function getProviderNodeInfo(providerUrl) {
  const res = await fetch(`${providerUrl}/info`, {
    method: "GET",
    headers: { "content-type": "application/json" },
  });
  if (!res.ok) {
    throw new Error(
      `/info failed: ${res.status} ${await res.text().catch(() => "")}`
    );
  }
  return res.json();
}

/**
 * Submit `establish_storage_agreement` with provider-signed terms.
 *
 * `signed.signature` is the SCALE-encoded `MultiSignature` as hex, e.g.
 * `0x00<64-byte-sig>` for Sr25519. Strip the variant prefix and wrap the
 * raw 64 bytes back into the PAPI Enum.
 */
const MULTI_SIGNATURE_VARIANT = Object.freeze({
  0: "Ed25519",
  1: "Sr25519",
  2: "ecdsa",
  3: "eth",
});

/**
  * Build the agreement terms and sign it
 */
export function buildSignedTermsArgs(provider, signed) {
  const sigBytes = hexToBytes(signed.signature);
  if (sigBytes.length < 1) {
    throw new Error("signature too short to contain a MultiSignature variant byte");
  }
  const variantIdx = sigBytes[0];
  const variantName = MULTI_SIGNATURE_VARIANT[variantIdx];
  if (!variantName) {
    throw new Error(`unknown MultiSignature variant byte: ${variantIdx}`);
  }
  const sig = Enum(variantName, Binary.fromBytes(sigBytes.slice(1)));

  const t = signed.terms;
  const terms = {
    owner: t.owner,
    max_bytes: BigInt(t.max_bytes),
    duration: t.duration,
    price_per_byte: BigInt(t.price_per_byte),
    valid_until: t.valid_until,
    nonce: BigInt(t.nonce),
    bucket_id: t.bucket_id != null ? BigInt(t.bucket_id) : undefined,
    replica_params: t.replica_params ?? undefined,
  };
  return { provider: provider.address, terms, sig };
}

export async function establishStorageAgreement(api, client, provider, signed) {
  const result = await submitTx(
    api.tx.StorageProvider.establish_storage_agreement(
      buildSignedTermsArgs(provider, signed)
    ),
    client.signer,
    "establish_storage_agreement"
  );
  const event = requireOneEvent(
    result.events,
    api.event.StorageProvider.BucketCreated,
    "BucketCreated"
  );
  return event.bucket_id;
}

export async function setMember(api, admin, bucketId, member, role) {
  return submitTx(
    api.tx.StorageProvider.set_member({
      bucket_id: bucketId,
      member: member.address,
      role: Enum(role),
    }),
    admin.signer,
    `set_member(${role})`
  );
}

export async function removeMember(api, admin, bucketId, member) {
  return submitTx(
    api.tx.StorageProvider.remove_member({
      bucket_id: bucketId,
      member: member.address,
    }),
    admin.signer,
    "remove_member"
  );
}

export async function submitClientCheckpoint(api, client, provider, bucketId, ck) {
  // `ck.nonce` is the block number the provider signed over (returned by
  // `fetchCheckpointSignature`). The on-chain pallet rejects signatures whose
  // nonce is older than `MaxNonceAge` so we pass it through here.
  return submitTx(
    api.tx.StorageProvider.checkpoint({
      bucket_id: bucketId,
      mmr_root: Binary.fromBytes(hexToBytes(ck.mmr_root)),
      start_seq: BigInt(ck.start_seq),
      leaf_count: BigInt(ck.leaf_count),
      nonce: BigInt(ck.nonce),
      signatures: [
        [
          provider.address,
          Enum("Sr25519", Binary.fromBytes(hexToBytes(ck.provider_signature))),
        ],
      ],
    }),
    client.signer,
    "checkpoint"
  );
}

export async function challengeOffchain(api, client, provider, bucketId, upload) {
  // Finalized: the challenge_id must survive to the respond that references it
  // (a best-block reorg would invalidate it -> ChallengeNotFound).
  const result = await submitTxFinalized(
    api.tx.StorageProvider.challenge_offchain({
      bucket_id: bucketId,
      provider: provider.address,
      mmr_root: Binary.fromBytes(hexToBytes(upload.mmrRoot)),
      start_seq: BigInt(upload.startSeq),
      // The pallet honours `leaf_count` in the reconstructed `CommitmentPayload`
      // (previously hardcoded 0). The provider returned the value it signed
      // over from /commit — pass it through verbatim.
      leaf_count: BigInt(upload.leafCount),
      leaf_index: BigInt(upload.leafIndex),
      chunk_index: 0n,
      nonce: BigInt(upload.nonce),
      provider_signature: Enum(
        "Sr25519",
        Binary.fromBytes(hexToBytes(upload.providerSignature))
      ),
    }),
    client.signer,
    "challenge_offchain"
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.ChallengeCreated,
    "ChallengeCreated (offchain)"
  ).challenge_id;
}

export async function challengeCheckpoint(api, client, provider, bucketId, leafIndex) {
  // Finalized: see challengeOffchain.
  const result = await submitTxFinalized(
    api.tx.StorageProvider.challenge_checkpoint({
      bucket_id: bucketId,
      provider: provider.address,
      leaf_index: BigInt(leafIndex),
      chunk_index: 0n,
    }),
    client.signer,
    "challenge_checkpoint"
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.ChallengeCreated,
    "ChallengeCreated (checkpoint)"
  ).challenge_id;
}

export async function respondToChallenge(api, provider, challengeId, proof) {
  return submitTx(
    api.tx.StorageProvider.respond_to_challenge({
      challenge_id: challengeId,
      response: Enum("Proof", proof),
    }),
    provider.signer,
    "respond_to_challenge"
  );
}

export async function endAgreement(api, client, provider, bucketId, action = "Pay", actionValue) {
  return submitTx(
    api.tx.StorageProvider.end_agreement({
      bucket_id: bucketId,
      provider: provider.address,
      action: actionValue !== undefined ? Enum(action, actionValue) : Enum(action),
    }),
    client.signer,
    `end_agreement(${action})`
  );
}

export async function addStake(api, provider, amount) {
  return submitTx(
    api.tx.StorageProvider.add_stake({ amount }),
    provider.signer,
    "add_stake"
  );
}

export async function deregisterProvider(api, provider) {
  return submitTx(
    api.tx.StorageProvider.deregister_provider(),
    provider.signer,
    "deregister_provider"
  );
}

export async function completeDeregister(api, provider) {
  return submitTx(
    api.tx.StorageProvider.complete_deregister(),
    provider.signer,
    "complete_deregister"
  );
}

export async function cancelDeregister(api, provider) {
  return submitTx(
    api.tx.StorageProvider.cancel_deregister(),
    provider.signer,
    "cancel_deregister"
  );
}

export async function updateProviderMultiaddr(api, provider, multiaddr) {
  const bytes = typeof multiaddr === "string"
    ? new TextEncoder().encode(multiaddr)
    : multiaddr;
  return submitTx(
    api.tx.StorageProvider.update_provider_multiaddr({
      multiaddr: Binary.fromBytes(bytes),
    }),
    provider.signer,
    "update_provider_multiaddr"
  );
}

export async function setMinProviders(api, admin, bucketId, minProviders) {
  return submitTx(
    api.tx.StorageProvider.set_min_providers({
      bucket_id: bucketId,
      min_providers: minProviders,
    }),
    admin.signer,
    "set_min_providers"
  );
}

export async function claimExpiredAgreement(api, caller, bucketId, provider) {
  return submitTx(
    api.tx.StorageProvider.claim_expired_agreement({
      bucket_id: bucketId,
      provider: provider.address,
    }),
    caller.signer,
    "claim_expired_agreement"
  );
}

export async function extendAgreement(api, client, bucketId, provider, params) {
  return submitTx(
    api.tx.StorageProvider.extend_agreement({
      bucket_id: bucketId,
      provider: provider.address,
      ...params,
    }),
    client.signer,
    "extend_agreement"
  );
}

export async function topUpAgreement(api, client, bucketId, provider, params) {
  return submitTx(
    api.tx.StorageProvider.top_up_agreement({
      bucket_id: bucketId,
      provider: provider.address,
      ...params,
    }),
    client.signer,
    "top_up_agreement"
  );
}

export async function setExtensionsBlocked(api, provider, bucketId, blocked) {
  return submitTx(
    api.tx.StorageProvider.set_extensions_blocked({
      bucket_id: bucketId,
      blocked,
    }),
    provider.signer,
    "set_extensions_blocked"
  );
}

export async function freezeBucket(api, client, bucketId) {
  const result = await submitTx(
    api.tx.StorageProvider.freeze_bucket({ bucket_id: bucketId }),
    client.signer,
    "freeze_bucket"
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.BucketFrozen,
    "BucketFrozen"
  );
}

export async function configureCheckpointWindow(api, admin, bucketId, { interval, gracePeriod, enabled = true }) {
  const result = await submitTx(
    api.tx.StorageProvider.configure_checkpoint_window({
      bucket_id: bucketId,
      interval,
      grace_period: gracePeriod,
      enabled,
    }),
    admin.signer,
    "configure_checkpoint_window"
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointConfigUpdated,
    "CheckpointConfigUpdated"
  );
}

export async function fundCheckpointPool(api, funder, bucketId, amount) {
  const result = await submitTx(
    api.tx.StorageProvider.fund_checkpoint_pool({
      bucket_id: bucketId,
      amount,
    }),
    funder.signer,
    "fund_checkpoint_pool"
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointPoolFunded,
    "CheckpointPoolFunded"
  );
}

export async function submitProviderCheckpoint(api, provider, bucketId, duty, signature, window) {
  const result = await submitTx(
    api.tx.StorageProvider.provider_checkpoint({
      bucket_id: bucketId,
      mmr_root: Binary.fromBytes(hexToBytes(duty.mmr_root)),
      start_seq: BigInt(duty.start_seq),
      leaf_count: BigInt(duty.leaf_count),
      window,
      signatures: [
        [
          provider.address,
          Enum("Sr25519", Binary.fromBytes(hexToBytes(signature))),
        ],
      ],
    }),
    provider.signer,
    "provider_checkpoint"
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.ProviderCheckpointSubmitted,
    "ProviderCheckpointSubmitted"
  );
}

export async function claimCheckpointRewards(api, provider, bucketId) {
  const result = await submitTx(
    api.tx.StorageProvider.claim_checkpoint_rewards({ bucket_id: bucketId }),
    provider.signer,
    "claim_checkpoint_rewards"
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointRewardClaimed,
    "CheckpointRewardClaimed"
  );
}

export async function reportMissedCheckpoint(api, reporter, bucketId, window) {
  const result = await submitTx(
    api.tx.StorageProvider.report_missed_checkpoint({
      bucket_id: bucketId,
      window,
    }),
    reporter.signer,
    "report_missed_checkpoint"
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointMissPenalized,
    "CheckpointMissPenalized"
  );
}

// ============================================================================
// DriveRegistry pallet (Layer 1 — file system)
// ============================================================================

/**
 * Create a drive by redeeming provider-signed terms.
 *
 * Layer 0's `establish_storage_agreement_internal` opens the underlying
 * bucket + primary agreement atomically inside `create_drive`.
 */
export async function createDrive(api, owner, name, provider, signed) {
  const result = await submitTx(
    api.tx.DriveRegistry.create_drive({
      name: Binary.fromBytes(new TextEncoder().encode(name)),
      ...buildSignedTermsArgs(provider, signed),
    }),
    owner.signer,
    "create_drive"
  );
  const event = requireOneEvent(
    result.events,
    api.event.DriveRegistry.DriveCreated,
    "DriveCreated"
  );
  return {
    driveId: event.drive_id,
    bucketId: event.bucket_id,
  };
}

export async function shareDrive(api, owner, driveId, member, role) {
  const result = await submitTx(
    api.tx.DriveRegistry.share_drive({
      drive_id: driveId,
      member: member.address,
      role: Enum(role),
    }),
    owner.signer,
    `share_drive(${role})`
  );
  return requireOneEvent(
    result.events,
    api.event.DriveRegistry.DriveShared,
    "DriveShared"
  );
}

export async function unshareDrive(api, owner, driveId, member) {
  const result = await submitTx(
    api.tx.DriveRegistry.unshare_drive({
      drive_id: driveId,
      member: member.address,
    }),
    owner.signer,
    "unshare_drive"
  );
  return requireOneEvent(
    result.events,
    api.event.DriveRegistry.DriveUnshared,
    "DriveUnshared"
  );
}

export async function deleteDrive(api, owner, driveId) {
  const result = await submitTx(
    api.tx.DriveRegistry.delete_drive({ drive_id: driveId }),
    owner.signer,
    "delete_drive"
  );
  return requireOneEvent(
    result.events,
    api.event.DriveRegistry.DriveDeleted,
    "DriveDeleted"
  );
}

// ============================================================================
// S3Registry pallet (Layer 1 — S3-style objects)
// ============================================================================

/**
 * Create an S3 bucket by redeeming provider-signed terms.
 *
 * Layer 0's `establish_storage_agreement_internal` opens the underlying
 * Layer 0 bucket + primary agreement atomically inside `create_s3_bucket`.
 */
export async function createS3Bucket(api, client, name, provider, signed) {
  const result = await submitTx(
    api.tx.S3Registry.create_s3_bucket({
      name: Binary.fromBytes(new TextEncoder().encode(name)),
      ...buildSignedTermsArgs(provider, signed),
    }),
    client.signer,
    "create_s3_bucket"
  );
  const event = requireOneEvent(
    result.events,
    api.event.S3Registry.S3BucketCreated,
    "S3BucketCreated"
  );
  return {
    s3BucketId: event.s3_bucket_id,
    layer0BucketId: event.layer0_bucket_id,
  };
}

export async function putObjectMetadata(api, client, s3BucketId, key, obj, contentType, userMetadata = []) {
  return submitTx(
    api.tx.S3Registry.put_object_metadata({
      s3_bucket_id: s3BucketId,
      key: Binary.fromBytes(new TextEncoder().encode(key)),
      cid: Binary.fromBytes(obj.cid),
      size: obj.size,
      content_type: Binary.fromBytes(new TextEncoder().encode(contentType)),
      user_metadata: userMetadata.map(([k, v]) => [
        Binary.fromBytes(new TextEncoder().encode(k)),
        Binary.fromBytes(new TextEncoder().encode(v)),
      ]),
    }),
    client.signer,
    `put_object_metadata(${key})`
  );
}

export async function copyObjectMetadata(api, client, srcBucketId, srcKey, dstBucketId, dstKey) {
  return submitTx(
    api.tx.S3Registry.copy_object_metadata({
      src_bucket_id: srcBucketId,
      src_key: Binary.fromBytes(new TextEncoder().encode(srcKey)),
      dst_bucket_id: dstBucketId,
      dst_key: Binary.fromBytes(new TextEncoder().encode(dstKey)),
    }),
    client.signer,
    `copy_object_metadata(${srcKey} -> ${dstKey})`
  );
}

export async function deleteObjectMetadata(api, client, s3BucketId, key) {
  return submitTx(
    api.tx.S3Registry.delete_object_metadata({
      s3_bucket_id: s3BucketId,
      key: Binary.fromBytes(new TextEncoder().encode(key)),
    }),
    client.signer,
    `delete_object_metadata(${key})`
  );
}

export async function deleteS3Bucket(api, client, s3BucketId) {
  const result = await submitTx(
    api.tx.S3Registry.delete_s3_bucket({ s3_bucket_id: s3BucketId }),
    client.signer,
    "delete_s3_bucket"
  );
  return requireOneEvent(
    result.events,
    api.event.S3Registry.S3BucketDeleted,
    "S3BucketDeleted"
  );
}

// ============================================================================
// Provider node HTTP helpers
//
// These are not pallet extrinsics, but they're the off-chain half of flows
// that the pallet wrappers above complete. Putting them here keeps the demo
// scripts focused on scenario logic rather than HTTP plumbing.
// ============================================================================

/**
 * PUT a single chunk to the provider without requesting an MMR commitment.
 * Suitable for S3-style object uploads where the Layer 1 metadata records
 * the CID itself and no Layer 0 checkpoint follows immediately. Returns
 * `{ hash, cid, size, data }` (hash is hex; cid is the raw Uint8Array).
 */
export async function putChunk(providerUrl, bucketId, data) {
  const bytes = data instanceof Uint8Array ? data : new TextEncoder().encode(data);
  const cid = blake2b256(bytes);
  const hash = toHex(cid);
  await providerFetch(providerUrl, "/node", {
    method: "PUT",
    body: {
      bucket_id: Number(bucketId),
      hash,
      data: Buffer.from(bytes).toString("base64"),
      children: null,
    },
  });
  return { hash, cid, size: BigInt(bytes.length), data: bytes };
}

/**
 * PUT a chunk to the provider and request an MMR commitment. Returns the
 * chunk hash, original bytes, and the /commit response (mmr_root, leaf_indices,
 * start_seq, provider_signature).
 */
export async function uploadChunk(providerUrl, bucketId, data, nonce) {
  // `nonce` is the block at which the caller intends to submit any extrinsic
  // that consumes the resulting `provider_signature` (`challenge_offchain`,
  // `checkpoint`, …). The pallet rejects signatures whose nonce is older than
  // `MaxNonceAge`, so capture a recent block and pass it through.
  if (typeof nonce !== "number" && typeof nonce !== "bigint") {
    throw new Error("uploadChunk requires a numeric `nonce` (block number)");
  }
  const bytes = data instanceof Uint8Array ? data : new TextEncoder().encode(data);
  const hash = toHex(blake2b256(bytes));
  await providerFetch(providerUrl, "/node", {
    method: "PUT",
    body: {
      bucket_id: Number(bucketId),
      hash,
      data: Buffer.from(bytes).toString("base64"),
      children: null,
    },
  });
  const commit = await providerFetch(providerUrl, "/commit", {
    method: "POST",
    body: {
      bucket_id: Number(bucketId),
      data_roots: [hash],
      nonce: Number(nonce),
    },
  });
  return { hash, data: bytes, commit };
}

export async function downloadChunk(providerUrl, chunkHashHex) {
  const downloaded = await providerFetch(providerUrl, "/node", {
    params: { hash: chunkHashHex },
  });
  return Buffer.from(downloaded.data, "base64");
}

export async function fetchCheckpointSignature(providerUrl, bucketId, nonce) {
  // The provider signs over the `CommitmentPayload` including the supplied
  // `nonce`. Pass the block the caller will submit at so the on-chain
  // recency check passes.
  if (typeof nonce !== "number" && typeof nonce !== "bigint") {
    throw new Error(
      "fetchCheckpointSignature requires a numeric `nonce` (block number)"
    );
  }
  return providerFetch(providerUrl, "/checkpoint-signature", {
    params: { bucket_id: Number(bucketId), nonce: Number(nonce) },
  });
}

export async function fetchCheckpointDuty(providerUrl, bucketId) {
  return providerFetch(providerUrl, "/checkpoint/duty", {
    params: { bucket_id: Number(bucketId) },
  });
}

export async function signCheckpointProposal(providerUrl, bucketId, duty, window) {
  return providerFetch(providerUrl, "/checkpoint/sign", {
    method: "POST",
    body: {
      bucket_id: Number(bucketId),
      mmr_root: duty.mmr_root,
      start_seq: duty.start_seq,
      leaf_count: duty.leaf_count,
      window: Number(window),
    },
  });
}

/**
 * Build the proof payload for `respond_to_challenge` by reading the challenge
 * from chain state and fetching MMR + chunk proofs from the provider node.
 */
export async function fetchChallengeProof(api, providerUrl, challengeId) {
  // Best block: a finalized read would lag the just-created challenge.
  // Challenges is a StorageDoubleMap keyed by (deadline, index), so the
  // single challenge is read directly with both keys.
  const challenge = await api.query.StorageProvider.Challenges.getValue(
    challengeId.deadline,
    challengeId.index,
    READ_OPTS
  );
  if (!challenge)
    throw new Error(
      "Challenge not found: deadline " +
        challengeId.deadline +
        " index " +
        challengeId.index
    );

  const mmr = await providerFetch(providerUrl, "/mmr_proof", {
    params: {
      bucket_id: Number(challenge.bucket_id),
      leaf_index: Number(challenge.leaf_index),
    },
  });
  const chunk = await providerFetch(providerUrl, "/chunk_proof", {
    params: {
      data_root: mmr.leaf.data_root,
      chunk_index: Number(challenge.chunk_index),
    },
  });

  return {
    chunk_data: Binary.fromBytes(Buffer.from(chunk.chunk_data, "base64")),
    mmr_proof: {
      peaks: mmr.proof.peaks.map((h) => Binary.fromBytes(hexToBytes(h))),
      leaf: {
        data_root: Binary.fromBytes(hexToBytes(mmr.leaf.data_root)),
        data_size: BigInt(mmr.leaf.data_size),
        total_size: BigInt(mmr.leaf.total_size),
      },
      leaf_proof: {
        siblings: mmr.proof.siblings.map((h) => Binary.fromBytes(hexToBytes(h))),
        path: mmr.proof.path,
      },
    },
    chunk_proof: {
      siblings: chunk.proof.siblings.map((h) => Binary.fromBytes(hexToBytes(h))),
      path: chunk.proof.path,
    },
  };
}
