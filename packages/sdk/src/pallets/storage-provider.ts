/**
 * Thin typed wrappers around the StorageProvider pallet extrinsics. Each
 * submits at in-block inclusion (see tx.ts) and extracts what callers need
 * from the tx result's events. The challenge creators are the documented
 * exception: they finalize, because a challenge id embeds its creation block
 * and a best-block reorg would invalidate it before the response lands.
 */

import { Enum } from "polkadot-api";

import { asHex, bytesEq, type ParachainApi } from "../address.js";
import type { ChainSigner } from "../signers.js";
import { READ_OPTS, requireOneEvent, submitTx, submitTxFinalized } from "../tx.js";

export async function registerProvider(
  api: ParachainApi,
  provider: ChainSigner,
  providerUrl: string,
  stake: bigint = 1_000_000_000_000_000n,
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
    "register_provider",
  );
}

export async function updateProviderSettings(
  api: ParachainApi,
  provider: ChainSigner,
  settings: any,
) {
  return submitTx(
    api.tx.StorageProvider.update_provider_settings({ settings }),
    provider.signer,
    "update_provider_settings",
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
) {
  const existing = await api.query.StorageProvider.Providers.getValue(
    provider.address,
    READ_OPTS,
  );
  if (!existing) {
    console.log("  Registering provider", provider.address);
    await registerProvider(api, provider, providerUrl);
  } else {
    if (!bytesEq(existing.public_key, provider.publicKey)) {
      throw new Error(
        `Provider ${provider.address} is already registered with a different public_key. ` +
          `Restart the chain, or run this script with a fresh provider seed.`,
      );
    }
  }
  // Always (re)apply settings so price/acceptance are correct for this run.
  await updateProviderSettings(api, provider, {
    min_duration: 10,
    max_duration: maxDuration,
    price_per_byte: pricePerByte,
    accepting_primary: true,
    replica_sync_price: undefined,
    accepting_extensions: true,
    max_capacity: 0n,
  });
}

export async function createBucket(
  api: ParachainApi,
  signer: ChainSigner,
  { minProviders = 1 }: { minProviders?: number } = {},
) {
  const result = await submitTx(
    api.tx.StorageProvider.create_bucket({ min_providers: minProviders }),
    signer.signer,
    "create_bucket",
  );
  const event = requireOneEvent(
    result.events,
    api.event.StorageProvider.BucketCreated,
    "BucketCreated",
  );
  return event.bucket_id;
}

export async function createBucketWithStorage(
  api: ParachainApi,
  client: ChainSigner,
  params: any,
) {
  const result = await submitTx(
    api.tx.StorageProvider.create_bucket_with_storage(params),
    client.signer,
    "create_bucket_with_storage",
  );
  const created = requireOneEvent(
    result.events,
    api.event.StorageProvider.BucketCreated,
    "BucketCreated",
  );
  const accepted = requireOneEvent(
    result.events,
    api.event.StorageProvider.AgreementAccepted,
    "AgreementAccepted",
  );
  return {
    bucketId: created.bucket_id,
    matchedProvider: accepted.provider,
    expiresAt: accepted.expires_at,
  };
}

export async function requestPrimaryAgreement(
  api: ParachainApi,
  client: ChainSigner,
  provider: ChainSigner | { address: string },
  bucketId: bigint,
  params: any,
) {
  return submitTx(
    api.tx.StorageProvider.request_primary_agreement({
      bucket_id: bucketId,
      provider: provider.address,
      ...params,
    }),
    client.signer,
    "request_primary_agreement",
  );
}

export async function acceptAgreement(
  api: ParachainApi,
  provider: ChainSigner,
  bucketId: bigint,
) {
  return submitTx(
    api.tx.StorageProvider.accept_agreement({ bucket_id: bucketId }),
    provider.signer,
    "accept_agreement",
  );
}

export async function setMember(
  api: ParachainApi,
  admin: ChainSigner,
  bucketId: bigint,
  member: ChainSigner | { address: string },
  role: string,
) {
  return submitTx(
    api.tx.StorageProvider.set_member({
      bucket_id: bucketId,
      member: member.address,
      role: Enum(role as never),
    }),
    admin.signer,
    `set_member(${role})`,
  );
}

export async function removeMember(
  api: ParachainApi,
  admin: ChainSigner,
  bucketId: bigint,
  member: ChainSigner | { address: string },
) {
  return submitTx(
    api.tx.StorageProvider.remove_member({
      bucket_id: bucketId,
      member: member.address,
    }),
    admin.signer,
    "remove_member",
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
) {
  return submitTx(
    api.tx.StorageProvider.checkpoint({
      bucket_id: bucketId,
      mmr_root: asHex(ck.mmr_root),
      start_seq: BigInt(ck.start_seq),
      leaf_count: BigInt(ck.leaf_count),
      signatures: [[provider.address, Enum("Sr25519", asHex(ck.provider_signature))]],
    }),
    client.signer,
    "checkpoint",
  );
}

export async function challengeOffchain(
  api: ParachainApi,
  client: ChainSigner,
  provider: ChainSigner | { address: string },
  bucketId: bigint,
  upload: {
    mmrRoot: string;
    startSeq: number | string;
    leafIndex: number | string;
    providerSignature: string;
  },
) {
  // Finalized: the challenge_id must survive to the respond that references it
  // (a best-block reorg would invalidate it -> ChallengeNotFound).
  const result = await submitTxFinalized(
    api.tx.StorageProvider.challenge_offchain({
      bucket_id: bucketId,
      provider: provider.address,
      mmr_root: asHex(upload.mmrRoot),
      start_seq: BigInt(upload.startSeq),
      leaf_index: BigInt(upload.leafIndex),
      chunk_index: 0n,
      provider_signature: Enum("Sr25519", asHex(upload.providerSignature)),
    }),
    client.signer,
    "challenge_offchain",
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
) {
  // Finalized: see challengeOffchain.
  const result = await submitTxFinalized(
    api.tx.StorageProvider.challenge_checkpoint({
      bucket_id: bucketId,
      provider: provider.address,
      leaf_index: BigInt(leafIndex),
      chunk_index: 0n,
    }),
    client.signer,
    "challenge_checkpoint",
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
) {
  return submitTx(
    api.tx.StorageProvider.respond_to_challenge({
      challenge_id: challengeId,
      response: Enum("Proof", proof),
    }),
    provider.signer,
    "respond_to_challenge",
  );
}

export async function endAgreement(
  api: ParachainApi,
  client: ChainSigner,
  provider: ChainSigner | { address: string },
  bucketId: bigint,
  action: string = "Pay",
  actionValue?: unknown,
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
    `end_agreement(${action})`,
  );
}

export async function addStake(
  api: ParachainApi,
  provider: ChainSigner,
  amount: bigint,
) {
  return submitTx(
    api.tx.StorageProvider.add_stake({ amount }),
    provider.signer,
    "add_stake",
  );
}

export async function deregisterProvider(api: ParachainApi, provider: ChainSigner) {
  return submitTx(
    api.tx.StorageProvider.deregister_provider(),
    provider.signer,
    "deregister_provider",
  );
}

export async function completeDeregister(api: ParachainApi, provider: ChainSigner) {
  return submitTx(
    api.tx.StorageProvider.complete_deregister(),
    provider.signer,
    "complete_deregister",
  );
}

export async function cancelDeregister(api: ParachainApi, provider: ChainSigner) {
  return submitTx(
    api.tx.StorageProvider.cancel_deregister(),
    provider.signer,
    "cancel_deregister",
  );
}

export async function updateProviderMultiaddr(
  api: ParachainApi,
  provider: ChainSigner,
  multiaddr: string | Uint8Array,
) {
  const bytes =
    typeof multiaddr === "string" ? new TextEncoder().encode(multiaddr) : multiaddr;
  return submitTx(
    api.tx.StorageProvider.update_provider_multiaddr({ multiaddr: bytes }),
    provider.signer,
    "update_provider_multiaddr",
  );
}

export async function setMinProviders(
  api: ParachainApi,
  admin: ChainSigner,
  bucketId: bigint,
  minProviders: number,
) {
  return submitTx(
    api.tx.StorageProvider.set_min_providers({
      bucket_id: bucketId,
      min_providers: minProviders,
    }),
    admin.signer,
    "set_min_providers",
  );
}

export async function rejectAgreement(
  api: ParachainApi,
  provider: ChainSigner,
  bucketId: bigint,
) {
  return submitTx(
    api.tx.StorageProvider.reject_agreement({ bucket_id: bucketId }),
    provider.signer,
    "reject_agreement",
  );
}

export async function withdrawAgreementRequest(
  api: ParachainApi,
  client: ChainSigner,
  bucketId: bigint,
  provider: ChainSigner | { address: string },
) {
  return submitTx(
    api.tx.StorageProvider.withdraw_agreement_request({
      bucket_id: bucketId,
      provider: provider.address,
    }),
    client.signer,
    "withdraw_agreement_request",
  );
}

export async function claimExpiredAgreement(
  api: ParachainApi,
  caller: ChainSigner,
  bucketId: bigint,
) {
  return submitTx(
    // The runtime derives the provider from the signed origin.
    api.tx.StorageProvider.claim_expired_agreement({ bucket_id: bucketId }),
    caller.signer,
    "claim_expired_agreement",
  );
}

export async function extendAgreement(
  api: ParachainApi,
  client: ChainSigner,
  bucketId: bigint,
  provider: ChainSigner | { address: string },
  params: any,
) {
  return submitTx(
    api.tx.StorageProvider.extend_agreement({
      bucket_id: bucketId,
      provider: provider.address,
      ...params,
    }),
    client.signer,
    "extend_agreement",
  );
}

export async function topUpAgreement(
  api: ParachainApi,
  client: ChainSigner,
  bucketId: bigint,
  provider: ChainSigner | { address: string },
  params: any,
) {
  return submitTx(
    api.tx.StorageProvider.top_up_agreement({
      bucket_id: bucketId,
      provider: provider.address,
      ...params,
    }),
    client.signer,
    "top_up_agreement",
  );
}

export async function setExtensionsBlocked(
  api: ParachainApi,
  provider: ChainSigner,
  bucketId: bigint,
  blocked: boolean,
) {
  return submitTx(
    api.tx.StorageProvider.set_extensions_blocked({
      bucket_id: bucketId,
      blocked,
    }),
    provider.signer,
    "set_extensions_blocked",
  );
}

export async function freezeBucket(
  api: ParachainApi,
  client: ChainSigner,
  bucketId: bigint,
) {
  const result = await submitTx(
    api.tx.StorageProvider.freeze_bucket({ bucket_id: bucketId }),
    client.signer,
    "freeze_bucket",
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.BucketFrozen,
    "BucketFrozen",
  );
}

export async function configureCheckpointWindow(
  api: ParachainApi,
  admin: ChainSigner,
  bucketId: bigint,
  {
    interval,
    gracePeriod,
    enabled = true,
  }: { interval: number; gracePeriod: number; enabled?: boolean },
) {
  const result = await submitTx(
    api.tx.StorageProvider.configure_checkpoint_window({
      bucket_id: bucketId,
      interval,
      grace_period: gracePeriod,
      enabled,
    }),
    admin.signer,
    "configure_checkpoint_window",
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointConfigUpdated,
    "CheckpointConfigUpdated",
  );
}

export async function fundCheckpointPool(
  api: ParachainApi,
  funder: ChainSigner,
  bucketId: bigint,
  amount: bigint,
) {
  const result = await submitTx(
    api.tx.StorageProvider.fund_checkpoint_pool({
      bucket_id: bucketId,
      amount,
    }),
    funder.signer,
    "fund_checkpoint_pool",
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointPoolFunded,
    "CheckpointPoolFunded",
  );
}

export async function submitProviderCheckpoint(
  api: ParachainApi,
  provider: ChainSigner,
  bucketId: bigint,
  duty: { mmr_root: string; start_seq: number | string; leaf_count: number | string },
  signature: string,
  window: number,
) {
  const result = await submitTx(
    api.tx.StorageProvider.provider_checkpoint({
      bucket_id: bucketId,
      mmr_root: asHex(duty.mmr_root),
      start_seq: BigInt(duty.start_seq),
      leaf_count: BigInt(duty.leaf_count),
      window: BigInt(window),
      signatures: [[provider.address, Enum("Sr25519", asHex(signature))]],
    }),
    provider.signer,
    "provider_checkpoint",
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.ProviderCheckpointSubmitted,
    "ProviderCheckpointSubmitted",
  );
}

export async function claimCheckpointRewards(
  api: ParachainApi,
  provider: ChainSigner,
  bucketId: bigint,
) {
  const result = await submitTx(
    api.tx.StorageProvider.claim_checkpoint_rewards({ bucket_id: bucketId }),
    provider.signer,
    "claim_checkpoint_rewards",
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointRewardClaimed,
    "CheckpointRewardClaimed",
  );
}

export async function reportMissedCheckpoint(
  api: ParachainApi,
  reporter: ChainSigner,
  bucketId: bigint,
  window: number,
) {
  const result = await submitTx(
    api.tx.StorageProvider.report_missed_checkpoint({
      bucket_id: bucketId,
      window: BigInt(window),
    }),
    reporter.signer,
    "report_missed_checkpoint",
  );
  return requireOneEvent(
    result.events,
    api.event.StorageProvider.CheckpointMissPenalized,
    "CheckpointMissPenalized",
  );
}
