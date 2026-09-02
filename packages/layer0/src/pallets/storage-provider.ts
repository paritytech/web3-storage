// SPDX-License-Identifier: Apache-2.0

/**
 * Thin typed wrappers around the StorageProvider pallet extrinsics. Each
 * submits at in-block inclusion (see tx.ts) and extracts what callers need
 * from the tx result's events. The challenge creators are the documented
 * exception: they finalize, because a challenge id embeds its creation block
 * and a best-block reorg would invalidate it before the response lands.
 */

import { Enum } from "polkadot-api";

import type { SignedTerms } from "@web3-storage/core";

import { asHex, bytesEq, hexToBytes, type ParachainApi } from "../address.js";
import { confirmDeletion, deleteData, getProviderNodeInfo } from "../provider-http.js";
import type { ChainSigner } from "../signers.js";
import { READ_OPTS, requireOneEvent, submitTx, submitTxFinalized, type SubmitOpts } from "../tx.js";

export async function registerProvider(
  api: ParachainApi,
  provider: ChainSigner,
  providerUrl: string,
  stake: bigint = 1_000_000_000_000_000n,
  opts: SubmitOpts = {},
) {
  const port = new URL(providerUrl).port;
  const multiaddr = new TextEncoder().encode(`/ip4/127.0.0.1/tcp/${port}`);
  return submitTx(
    api.tx.StorageProvider.register_provider({
      multiaddr,
      public_key: provider.publicKey,
      stake,
    }),
    provider.signer,
    { label: "register_provider", ...opts },
  );
}

export async function updateProviderSettings(
  api: ParachainApi,
  provider: ChainSigner,
  settings: any,
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.StorageProvider.update_provider_settings({ settings }),
    provider.signer,
    { label: "update_provider_settings", ...opts },
  );
}

/**
 * Ensure a provider is registered and configured to accept primary
 * agreements. Idempotent: safe to call when an earlier script on the same
 * chain already registered the provider. Fails loudly when the on-chain
 * public key differs from the signer's — off-chain signatures would fail to
 * verify much later otherwise.
 */
export async function ensureProviderRegistered(
  api: ParachainApi,
  provider: ChainSigner,
  providerUrl: string,
  {
    pricePerByte = 1n,
    maxDuration = 100_000,
  }: { pricePerByte?: bigint; maxDuration?: number } = {},
  opts: SubmitOpts = {},
) {
  const existing = await api.query.StorageProvider.Providers.getValue(
    provider.address,
    READ_OPTS,
  );
  if (!existing) {
    console.log("  Registering provider", provider.address);
    await registerProvider(api, provider, providerUrl, undefined, opts);
  } else {
    if (!bytesEq(existing.public_key, provider.publicKey)) {
      throw new Error(
        `Provider ${provider.address} is already registered with a different public_key. ` +
          `Restart the chain, or run this script with a fresh provider seed.`,
      );
    }
  }
  // Always (re)apply settings so price/acceptance are correct for this run.
  await updateProviderSettings(
    api,
    provider,
    {
      min_duration: 10,
      max_duration: maxDuration,
      price_per_byte: pricePerByte,
      accepting_primary: true,
      replica_sync_price: undefined,
      accepting_extensions: true,
      max_capacity: 0n,
    },
    opts,
  );

  // The provider node keeps its chain state (signing key, nonce counter,
  // registered settings) live from finalized blocks and rejects /negotiate
  // with 503 ChainStateNotReady until it has synced. Wait for it to catch up
  // to this registration + price so the very next negotiate succeeds.
  const MAX_ATTEMPTS = 20; // 20 × 3s = 60s
  for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
    const { readiness, provider_registration_info } = await getProviderNodeInfo(providerUrl);
    const priceSynced =
      readiness.provider_info_loaded &&
      provider_registration_info != null &&
      BigInt(provider_registration_info.price_per_byte) === pricePerByte;
    if (readiness.signing_configured && readiness.nonce_counter_ready && priceSynced) return;
    await new Promise((resolve) => setTimeout(resolve, 3000));
  }
  throw new Error(
    `Provider node at ${providerUrl} did not sync this registration within ` +
      `${MAX_ATTEMPTS * 3}s (still reporting not-ready from /info)`,
  );
}

const MULTI_SIGNATURE_VARIANT: Record<number, string> = {
  0: "Ed25519",
  1: "Sr25519",
  2: "ecdsa",
  3: "eth",
};

/**
 * Shape a provider's SignedTerms into the `{ provider, terms, sig }` argument
 * the establish_* extrinsics (and create_drive / create_s3_bucket) expect. The
 * signature arrives as a SCALE-encoded MultiSignature hex; strip its variant
 * byte and re-wrap the raw bytes into the PAPI Enum.
 */
export function buildSignedTermsArgs(
  provider: ChainSigner | { address: string },
  signed: SignedTerms,
) {
  const sigBytes = hexToBytes(signed.signature);
  if (sigBytes.length < 1) {
    throw new Error("signature too short to contain a MultiSignature variant byte");
  }
  const variantName = MULTI_SIGNATURE_VARIANT[sigBytes[0]];
  if (!variantName) {
    throw new Error(`unknown MultiSignature variant byte: ${sigBytes[0]}`);
  }
  // The Sr25519 variant value is a fixed [u8; 64] — PAPI v2 takes it as 0x-hex
  // (asHex), the same convention the checkpoint/challenge wrappers use.
  const sig = Enum(variantName as never, asHex(sigBytes.slice(1)));
  const t = signed.terms;
  const terms = {
    owner: t.owner,
    max_bytes: BigInt(t.max_bytes),
    duration: t.duration,
    price_per_byte: BigInt(t.price_per_byte),
    valid_until: t.valid_until,
    nonce: BigInt(t.nonce),
    bucket_id: t.bucket_id != null ? BigInt(t.bucket_id) : undefined,
    replica_params: t.replica_params
      ? {
          sync_balance: BigInt(t.replica_params.sync_balance),
          min_sync_interval: t.replica_params.min_sync_interval,
          // Always present on provider-signed terms (the runtime requires it);
          // the `?`-typed field is only optional for the negotiate *request*.
          sync_price: BigInt(t.replica_params.sync_price ?? 0),
        }
      : undefined,
  };
  return { provider: provider.address, terms, sig };
}

/**
 * Redeem provider-signed terms via `establish_storage_agreement`: opens the
 * bucket and its primary agreement atomically. Replaces the old create_bucket
 * + request_agreement + accept_agreement dance (#105). `signed` comes from
 * {@link negotiateTerms} against the provider's /negotiate endpoint.
 */
export async function establishStorageAgreement(
  api: ParachainApi,
  client: ChainSigner,
  provider: ChainSigner | { address: string },
  signed: SignedTerms,
  opts: SubmitOpts = {},
) {
  const result = await submitTx(
    api.tx.StorageProvider.establish_storage_agreement(buildSignedTermsArgs(provider, signed)),
    client.signer,
    { label: "establish_storage_agreement", ...opts },
  );
  const created = requireOneEvent(
    result.events,
    api.event.StorageProvider.BucketCreated,
    "BucketCreated",
  );
  const established = requireOneEvent(
    result.events,
    api.event.StorageProvider.StorageAgreementEstablished,
    "StorageAgreementEstablished",
  );
  return {
    bucketId: created.bucket_id,
    provider: provider.address,
    expiresAt: established.expires_at,
  };
}

/**
 * Redeem provider-signed replica terms via `establish_replica_agreement`:
 * attaches a replica agreement to the bucket named in the signed terms
 * (`signed.terms.bucket_id`), using the same off-chain quote flow.
 */
export async function establishReplicaAgreement(
  api: ParachainApi,
  client: ChainSigner,
  provider: ChainSigner | { address: string },
  signed: SignedTerms,
  opts: SubmitOpts = {},
) {
  const args = buildSignedTermsArgs(provider, signed);
  if (args.terms.bucket_id == null) {
    throw new Error("replica agreement terms must carry a bucket_id");
  }
  const result = await submitTx(
    api.tx.StorageProvider.establish_replica_agreement({
      bucket_id: args.terms.bucket_id,
      ...args,
    }),
    client.signer,
    { label: "establish_replica_agreement", ...opts },
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.ReplicaAgreementEstablished,
    "ReplicaAgreementEstablished",
  );
}

export async function setMember(
  api: ParachainApi,
  admin: ChainSigner,
  bucketId: bigint,
  member: ChainSigner | { address: string },
  role: string,
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.StorageProvider.set_member({
      bucket_id: bucketId,
      member: member.address,
      role: Enum(role as never),
    }),
    admin.signer,
    { label: `set_member(${role})`, ...opts },
  );
}

export async function removeMember(
  api: ParachainApi,
  admin: ChainSigner,
  bucketId: bigint,
  member: ChainSigner | { address: string },
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.StorageProvider.remove_member({
      bucket_id: bucketId,
      member: member.address,
    }),
    admin.signer,
    { label: "remove_member", ...opts },
  );
}

export async function submitClientCheckpoint(
  api: ParachainApi,
  client: ChainSigner,
  provider: ChainSigner | { address: string },
  bucketId: bigint,
  ck: {
    mmr_root: string;
    start_seq: number | string;
    leaf_count: number | string;
    provider_signature: string;
  },
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.StorageProvider.checkpoint({
      bucket_id: bucketId,
      commitment: {
        mmr_root: asHex(ck.mmr_root),
        start_seq: BigInt(ck.start_seq),
        leaf_count: BigInt(ck.leaf_count),
      },
      signatures: [[provider.address, Enum("Sr25519", asHex(ck.provider_signature))]],
    }),
    client.signer,
    { label: "checkpoint", ...opts },
  );
}

/**
 * Prune a bucket's history below `newStartSeq` and make it canonical:
 * `POST /delete` on the provider (Admin-signed), hand back the admin's
 * signed deletion authorization (`/delete/confirm` — the on-chain `Deleted`
 * challenge defense), then submit the returned provider-signed commitment
 * via `checkpoint`. Once the checkpoint lands and the receipt is held, the
 * provider physically erases the pruned bytes and the quota headroom
 * returns.
 *
 * Bundled because a prune alone leaves the deletion in limbo: without the
 * checkpoint the on-chain liability never shrinks and the erasure
 * conditions can never be met, and without the confirmation the receipt is
 * missing — so the three calls are one logical operation. The pieces stay
 * exported individually for flows this helper cannot serve — notably
 * multi-primary buckets, where `checkpoint` needs `minProviders`
 * signatures: call `deleteData` per provider and submit one aggregate
 * checkpoint. This helper is the single-primary path.
 *
 * `client` must be a bucket Admin with a raw-signing keypair (the deletion
 * authorization cannot be produced by wallet-extension signers); `provider`
 * is the primary whose signature backs the checkpoint.
 */
export async function pruneAndCheckpoint(
  api: ParachainApi,
  providerUrl: string,
  client: ChainSigner,
  provider: ChainSigner | { address: string },
  bucketId: bigint,
  newStartSeq: bigint | number,
  opts: SubmitOpts = {},
) {
  const ck = await deleteData(providerUrl, bucketId, newStartSeq, client);
  await confirmDeletion(providerUrl, bucketId, ck, client);
  return submitClientCheckpoint(api, client, provider, bucketId, ck, opts);
}

export async function challengeOffchain(
  api: ParachainApi,
  client: ChainSigner,
  provider: ChainSigner | { address: string },
  bucketId: bigint,
  upload: {
    mmrRoot: string;
    startSeq: number | string;
    leafCount: number | string;
    leafIndex: number | string;
    providerSignature: string;
  },
  opts: SubmitOpts = {},
) {
  // Finalized: the challenge_id must survive to the respond that references it
  // (a best-block reorg would invalidate it -> ChallengeNotFound).
  const result = await submitTxFinalized(
    api.tx.StorageProvider.challenge_offchain({
      bucket_id: bucketId,
      provider: provider.address,
      // `commitment` is the (mmr_root, start_seq, leaf_count) the provider
      // signed over in /commit; passed through verbatim so the pallet's
      // CommitmentPayload reconstruction matches the signature.
      commitment: {
        mmr_root: asHex(upload.mmrRoot),
        start_seq: BigInt(upload.startSeq),
        leaf_count: BigInt(upload.leafCount),
      },
      // `target` is the leaf+chunk being challenged within that commitment.
      target: {
        leaf_index: BigInt(upload.leafIndex),
        chunk_index: 0n,
      },
      provider_signature: Enum("Sr25519", asHex(upload.providerSignature)),
    }),
    client.signer,
    { label: "challenge_offchain", ...opts },
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.ChallengeCreated,
    "ChallengeCreated (offchain)",
  ).challenge_id;
}

export async function challengeCheckpoint(
  api: ParachainApi,
  client: ChainSigner,
  provider: ChainSigner | { address: string },
  bucketId: bigint,
  leafIndex: number | bigint,
  opts: SubmitOpts = {},
) {
  // Finalized: see challengeOffchain.
  const result = await submitTxFinalized(
    api.tx.StorageProvider.challenge_checkpoint({
      bucket_id: bucketId,
      provider: provider.address,
      target: {
        leaf_index: BigInt(leafIndex),
        chunk_index: 0n,
      },
    }),
    client.signer,
    { label: "challenge_checkpoint", ...opts },
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.ChallengeCreated,
    "ChallengeCreated (checkpoint)",
  ).challenge_id;
}

export async function respondToChallenge(
  api: ParachainApi,
  provider: ChainSigner,
  challengeId: any,
  proof: any,
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.StorageProvider.respond_to_challenge({
      challenge_id: challengeId,
      response: Enum("Proof", proof),
    }),
    provider.signer,
    { label: "respond_to_challenge", ...opts },
  );
}

export async function endAgreement(
  api: ParachainApi,
  client: ChainSigner,
  provider: ChainSigner | { address: string },
  bucketId: bigint,
  action: string = "Pay",
  actionValue?: unknown,
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.StorageProvider.end_agreement({
      bucket_id: bucketId,
      provider: provider.address,
      action:
        actionValue !== undefined
          ? Enum(action as never, actionValue as never)
          : Enum(action as never),
    }),
    client.signer,
    { label: `end_agreement(${action})`, ...opts },
  );
}

export async function addStake(
  api: ParachainApi,
  provider: ChainSigner,
  amount: bigint,
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.StorageProvider.add_stake({ amount }),
    provider.signer,
    { label: "add_stake", ...opts },
  );
}

export async function deregisterProvider(
  api: ParachainApi,
  provider: ChainSigner,
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.StorageProvider.deregister_provider(),
    provider.signer,
    { label: "deregister_provider", ...opts },
  );
}

export async function completeDeregister(
  api: ParachainApi,
  provider: ChainSigner,
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.StorageProvider.complete_deregister(),
    provider.signer,
    { label: "complete_deregister", ...opts },
  );
}

export async function cancelDeregister(
  api: ParachainApi,
  provider: ChainSigner,
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.StorageProvider.cancel_deregister(),
    provider.signer,
    { label: "cancel_deregister", ...opts },
  );
}

export async function updateProviderMultiaddr(
  api: ParachainApi,
  provider: ChainSigner,
  multiaddr: string | Uint8Array,
  opts: SubmitOpts = {},
) {
  const bytes =
    typeof multiaddr === "string" ? new TextEncoder().encode(multiaddr) : multiaddr;
  return submitTx(
    api.tx.StorageProvider.update_provider_multiaddr({ multiaddr: bytes }),
    provider.signer,
    { label: "update_provider_multiaddr", ...opts },
  );
}

export async function setMinProviders(
  api: ParachainApi,
  admin: ChainSigner,
  bucketId: bigint,
  minProviders: number,
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.StorageProvider.set_min_providers({
      bucket_id: bucketId,
      min_providers: minProviders,
    }),
    admin.signer,
    { label: "set_min_providers", ...opts },
  );
}

export async function claimExpiredAgreement(
  api: ParachainApi,
  caller: ChainSigner,
  bucketId: bigint,
  opts: SubmitOpts = {},
) {
  return submitTx(
    // The runtime derives the provider from the signed origin.
    api.tx.StorageProvider.claim_expired_agreement({ bucket_id: bucketId }),
    caller.signer,
    { label: "claim_expired_agreement", ...opts },
  );
}

export async function extendAgreement(
  api: ParachainApi,
  client: ChainSigner,
  bucketId: bigint,
  provider: ChainSigner | { address: string },
  params: any,
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.StorageProvider.extend_agreement({
      bucket_id: bucketId,
      provider: provider.address,
      ...params,
    }),
    client.signer,
    { label: "extend_agreement", ...opts },
  );
}

export async function topUpAgreement(
  api: ParachainApi,
  client: ChainSigner,
  bucketId: bigint,
  provider: ChainSigner | { address: string },
  params: any,
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.StorageProvider.top_up_agreement({
      bucket_id: bucketId,
      provider: provider.address,
      ...params,
    }),
    client.signer,
    { label: "top_up_agreement", ...opts },
  );
}

export async function setExtensionsBlocked(
  api: ParachainApi,
  provider: ChainSigner,
  bucketId: bigint,
  blocked: boolean,
  opts: SubmitOpts = {},
) {
  return submitTx(
    api.tx.StorageProvider.set_extensions_blocked({
      bucket_id: bucketId,
      blocked,
    }),
    provider.signer,
    { label: "set_extensions_blocked", ...opts },
  );
}

export async function freezeBucket(
  api: ParachainApi,
  client: ChainSigner,
  bucketId: bigint,
  opts: SubmitOpts = {},
) {
  const result = await submitTx(
    api.tx.StorageProvider.freeze_bucket({ bucket_id: bucketId }),
    client.signer,
    { label: "freeze_bucket", ...opts },
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.BucketFrozen,
    "BucketFrozen",
  );
}
