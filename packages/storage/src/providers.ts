// SPDX-License-Identifier: Apache-2.0

/**
 * Provider discovery and checkpoint reads for UIs: every registered provider,
 * providers scored against storage requirements, and a bucket's latest
 * checkpoint.
 */

import { toSs58, type ParachainApi } from "./address.js";

export interface CheckpointInfo {
  mmrRoot: string;
  startSeq: bigint;
  leafCount: bigint;
  checkpointBlock: number;
}

/**
 * A registered provider, from the `StorageProviderApi.providers` runtime API.
 */
export interface AvailableProvider {
  account: string;
  multiaddr: string;
  stake: bigint;
  /** Free capacity per the chain; `undefined` = unlimited, not `0n` ("full"). */
  availableCapacity: bigint | undefined;
  maxCapacity: bigint;
  pricePerByte: bigint;
  minDuration: number;
  maxDuration: number;
  acceptingPrimary: boolean;
  /** Smallest agreement `max_bytes` the provider accepts; `0n` = no minimum. */
  minAgreementBytes: bigint;
  agreementsTotal: number;
}

/**
 * A provider scored against storage requirements via the
 * `find_matching_providers` runtime API. Carries a `matchScore` and
 * `partialReason` so the picker can rank "best fit" vs near-misses.
 */
export interface MatchingProviders extends AvailableProvider {
  matchScore: number;
  partialReason: string;
  committedBytes: bigint;
  replicaSyncPrice?: bigint;
  acceptingExtensions: boolean;
  registeredAt: number;
  agreementsExtended: number;
  agreementsNotExtended: number;
  agreementsBurned: number;
  /** Challenges resolved in the provider's favor (authorized + public tiers). */
  challengesDefended: number;
  challengesFailed: number;
  /** 0-100, computed on-chain by `ProviderStats::reputation`. */
  reputation: number;
}

export interface QueryMatchingProvidersParams {
  query: {
    bytesNeeded: bigint;
    minDuration: number;
    maxPricePerByte: bigint;
    primaryOnly: boolean;
  };
  limit: number;
}

/**
 * Limit for the `providers` and `find_matching_providers` runtime APIs that
 * returns every registered provider. One page for everything: paging over the
 * hash-ordered `Providers` map skips or repeats a provider when a
 * registration or deregistration occurs between two pages, and there is no
 * block handle to pin the pages to.
 */
export const ALL_PROVIDERS = 0xffff_ffff;

/** Largest u128: a `find_matching_providers` price cap that excludes no provider. */
export const NO_PRICE_CAP = (1n << 128n) - 1n;

/** Most free capacity first; unlimited (`undefined`) sorts ahead of metered. */
export function byFreeCapacityDesc(a: AvailableProvider, b: AvailableProvider): number {
  if (a.availableCapacity === b.availableCapacity) return 0;
  if (a.availableCapacity === undefined) return -1;
  if (b.availableCapacity === undefined) return 1;
  return b.availableCapacity > a.availableCapacity ? 1 : -1;
}

/** Every registered provider, most free capacity first. */
export async function listProviders(api: ParachainApi): Promise<AvailableProvider[]> {
  const page = await api.apis.StorageProviderApi.providers(0, ALL_PROVIDERS);

  const providers: AvailableProvider[] = page.map(([account, info]) => ({
    account,
    multiaddr: new TextDecoder().decode(info.multiaddr),
    stake: info.stake,
    // The chain already computed this; `null` is its "unlimited".
    availableCapacity:
      info.available_capacity == null ? undefined : BigInt(info.available_capacity),
    maxCapacity: BigInt(info.max_capacity),
    pricePerByte: info.price_per_byte,
    minDuration: info.min_duration,
    maxDuration: info.max_duration,
    acceptingPrimary: info.accepting_primary,
    minAgreementBytes: info.min_agreement_bytes,
    agreementsTotal: info.stats.agreements_total,
  }));

  providers.sort(byFreeCapacityDesc);

  return providers;
}

/**
 * Score registered providers against storage requirements with the
 * `find_matching_providers` runtime API, best match first.
 */
export async function findMatchingProviders(
  api: ParachainApi,
  query: QueryMatchingProvidersParams["query"],
  limit: QueryMatchingProvidersParams["limit"],
): Promise<MatchingProviders[]> {
  const matches = await api.apis.StorageProviderApi.find_matching_providers(
    {
      bytes_needed: query.bytesNeeded,
      min_duration: query.minDuration,
      max_price_per_byte: query.maxPricePerByte,
      primary_only: query.primaryOnly,
    },
    limit,
  );

  return matches
    .map((match) => {
      const info = match.info;
      const maxCapacity = BigInt(info.max_capacity ?? 0);
      const committedBytes = BigInt(info.committed_bytes ?? 0);
      // The chain already computed this; `null` is its "unlimited".
      const availableCapacity =
        info.available_capacity == null ? undefined : BigInt(info.available_capacity);

      return {
        account: toSs58(match.account),
        multiaddr: new TextDecoder().decode(info.multiaddr),
        stake: BigInt(info.stake ?? 0),
        availableCapacity,
        maxCapacity,
        committedBytes,
        pricePerByte: BigInt(info.price_per_byte ?? 0),
        minDuration: info.min_duration ?? 0,
        maxDuration: info.max_duration ?? 0,
        acceptingPrimary: info.accepting_primary ?? false,
        minAgreementBytes: BigInt(info.min_agreement_bytes ?? 0),
        replicaSyncPrice:
          info.replica_sync_price != null ? BigInt(info.replica_sync_price) : undefined,
        acceptingExtensions: info.accepting_extensions ?? false,
        registeredAt: Number(info.stats.registered_at ?? 0),
        agreementsTotal: info.stats.agreements_total ?? 0,
        agreementsExtended: info.stats.agreements_extended ?? 0,
        agreementsNotExtended: info.stats.agreements_not_extended ?? 0,
        agreementsBurned: info.stats.agreements_burned ?? 0,
        challengesDefended:
          (info.stats.challenges_received_authorized ?? 0) +
          (info.stats.challenges_received_public ?? 0),
        challengesFailed: info.stats.challenges_failed ?? 0,
        reputation: info.stats.reputation,
        matchScore: match.match_score,
        partialReason: match.partial_reason?.type ?? "",
      };
    })
    .sort((a, b) => b.matchScore - a.matchScore);
}

/** The bucket's latest checkpoint, or `null` before the first one. */
export async function getCheckpointInfo(api: ParachainApi, bucketId: bigint): Promise<CheckpointInfo | null> {
  const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
  if (!bucket) throw new Error(`Bucket ${bucketId} not found`);

  const snapshot = bucket.snapshot;
  if (!snapshot) return null;

  return {
    mmrRoot: snapshot.commitment.mmr_root,
    startSeq: snapshot.commitment.start_seq,
    leafCount: snapshot.commitment.leaf_count,
    checkpointBlock: snapshot.checkpoint_block,
  };
}
