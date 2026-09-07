// SPDX-License-Identifier: GPL-3.0-only

/**
 * Explorer data layer — one-shot network-wide snapshots of on-chain state.
 *
 * Everything is read via full-map `getEntries()` scans, matching how the
 * other apps read chain state. Fine at current network sizes; the pallet's
 * paginated runtime APIs (`StorageProviderApi.providers(offset, limit)` etc.)
 * are the upgrade path when scans get expensive.
 */

import { requireApi } from '@/lib/chain-client'

// ─────────────────────────────────────────────────────────────────────────────
// Row types
// ─────────────────────────────────────────────────────────────────────────────

/**
 * The on-chain `ProviderSettings` struct, verbatim — exactly the 7 fields the
 * pallet stores (see issue #122). `replicaSyncPrice` is the pallet's Option:
 * `undefined` means the provider does not accept replica agreements; there is
 * no separate "accepting replica" flag on chain.
 */
export interface ProviderSettings {
  minDuration: number
  maxDuration: number
  pricePerByte: bigint
  acceptingPrimary: boolean
  replicaSyncPrice: bigint | undefined
  acceptingExtensions: boolean
  /** 0 = unlimited */
  maxCapacity: bigint
}

export interface ProviderStats {
  registeredAt: number
  agreementsTotal: number
  agreementsExtended: number
  agreementsNotExtended: number
  agreementsBurned: number
  /** Lifetime cumulative quota ever committed — NOT current usage. */
  totalBytesCommitted: bigint
  /** Successfully defended challenges from authorized (member/owner) challengers. */
  challengesDefendedAuthorized: number
  /** Successfully defended challenges from general-public challengers. */
  challengesDefendedPublic: number
  /** Challenges the provider lost (slashed). */
  challengesFailed: number
}

export interface ProviderRow {
  address: string
  multiaddr: string
  stake: bigint
  /** Bytes currently under agreement (the pallet keeps this in sync). */
  committedBytes: bigint
  settings: ProviderSettings
  stats: ProviderStats
  deregisterAt: number | undefined
}

export interface AgreementRow {
  bucketId: number
  provider: string
  owner: string
  maxBytes: bigint
  paymentLocked: bigint
  pricePerByte: bigint
  /** Anchor-clock block; compare against the anchor block, never parachain height. */
  expiresAt: number
  startedAt: number
  role: string
  extensionsBlocked: boolean
}

export type AgreementStatus = 'active' | 'expired'

export interface BucketMember {
  account: string
  role: string
}

export interface BucketRow {
  id: number
  members: BucketMember[]
  minProviders: number
  primaryProviders: string[]
  hasSnapshot: boolean
  totalSnapshots: number
  frozen: boolean
  /**
   * Read visibility. 'Private' asks honest primaries to serve reads only to
   * members — a cooperative request, not on-chain enforced (replicas serve
   * everyone regardless). `undefined` on runtimes that predate the field.
   */
  visibility: 'Public' | 'Private' | undefined
}

export interface ChallengeRow {
  /** Anchor-clock deadline (first key of the Challenges double map). */
  deadline: number
  index: number
  bucketId: number
  provider: string
  challenger: string
  leafIndex: number
  chunkIndex: number
  deposit: bigint
  /** Challenger was a bucket member / agreement owner at creation (affects the fee split). */
  authorized: boolean
}

/**
 * Network-wide outcome counters summed over `ChallengerStats`. Resolved
 * challenges are deleted from storage, so these aggregates (plus events) are
 * the only durable outcome record — per-challenge history is issue #238.
 */
export interface ChallengeAggregates {
  totalIssued: number
  /** Provider slashed. */
  upheld: number
  /** Provider defended. */
  dismissed: number
}

export interface NetworkSnapshot {
  providers: ProviderRow[]
  agreements: AgreementRow[]
  buckets: BucketRow[]
  openChallenges: ChallengeRow[]
  challengeAggregates: ChallengeAggregates
  /** NextBucketId — buckets ever created (deleted ones included). */
  bucketsEverCreated: number
  /** Sections whose query failed (e.g. storage item missing on an older runtime). */
  failedSections: string[]
  fetchedAt: number
}

// ─────────────────────────────────────────────────────────────────────────────
// Snapshot loading
// ─────────────────────────────────────────────────────────────────────────────

export async function loadNetworkSnapshot(): Promise<NetworkSnapshot> {
  const api = requireApi()
  const failedSections: string[] = []

  // A query failing (most likely a storage item missing on an older live
  // runtime) degrades its own section instead of blanking the whole app.
  async function safe<T>(section: string, fallback: T, run: () => Promise<T>): Promise<T> {
    try {
      return await run()
    } catch (e) {
      console.warn(`explorer: failed to load ${section}:`, e)
      failedSections.push(section)
      return fallback
    }
  }

  const [providers, agreements, buckets, openChallenges, challengeAggregates, bucketsEverCreated] =
    await Promise.all([
      safe('providers', [] as ProviderRow[], async () => {
        const entries = await api.query.StorageProvider.Providers.getEntries()
        return entries.map(({ keyArgs, value }) => ({
          address: keyArgs[0],
          multiaddr: new TextDecoder().decode(value.multiaddr),
          stake: value.stake,
          committedBytes: BigInt(value.committed_bytes),
          settings: {
            minDuration: value.settings.min_duration,
            maxDuration: value.settings.max_duration,
            pricePerByte: value.settings.price_per_byte,
            acceptingPrimary: value.settings.accepting_primary,
            replicaSyncPrice: value.settings.replica_sync_price,
            acceptingExtensions: value.settings.accepting_extensions,
            maxCapacity: BigInt(value.settings.max_capacity),
          },
          stats: {
            registeredAt: value.stats.registered_at,
            agreementsTotal: value.stats.agreements_total,
            agreementsExtended: value.stats.agreements_extended,
            agreementsNotExtended: value.stats.agreements_not_extended,
            agreementsBurned: value.stats.agreements_burned,
            totalBytesCommitted: BigInt(value.stats.total_bytes_committed),
            // ?? 0: on a runtime predating the authorized/public tier split
            // the fields are absent; missing must not read as a render crash.
            challengesDefendedAuthorized: value.stats.challenges_received_authorized ?? 0,
            challengesDefendedPublic: value.stats.challenges_received_public ?? 0,
            challengesFailed: value.stats.challenges_failed,
          },
          deregisterAt: value.deregister_at ?? undefined,
        }))
      }),

      safe('agreements', [] as AgreementRow[], async () => {
        const entries = await api.query.StorageProvider.StorageAgreements.getEntries()
        return entries.map(({ keyArgs, value }) => ({
          bucketId: Number(keyArgs[0]),
          provider: keyArgs[1],
          owner: value.owner,
          maxBytes: BigInt(value.max_bytes),
          paymentLocked: value.payment_locked,
          pricePerByte: value.price_per_byte,
          expiresAt: value.expires_at,
          startedAt: value.started_at,
          role: value.role.type,
          extensionsBlocked: value.extensions_blocked,
        }))
      }),

      safe('buckets', [] as BucketRow[], async () => {
        const entries = await api.query.StorageProvider.Buckets.getEntries()
        return entries.map(({ keyArgs, value }) => ({
          id: Number(keyArgs[0]),
          members: value.members.map((m) => ({ account: m.account, role: m.role.type })),
          minProviders: value.min_providers,
          primaryProviders: value.primary_providers,
          hasSnapshot: value.snapshot !== undefined,
          totalSnapshots: value.total_snapshots,
          frozen: value.frozen_start_seq !== undefined,
          // Optional chain: absent on runtimes predating the field, and one
          // missing badge must not cost the whole buckets section.
          visibility: value.visibility?.type,
        }))
      }),

      safe('challenges', [] as ChallengeRow[], async () => {
        // Rows are deleted on resolution, so every entry is an open challenge.
        const entries = await api.query.StorageProvider.Challenges.getEntries()
        return entries
          .map(({ keyArgs, value }) => ({
            deadline: Number(keyArgs[0]),
            index: Number(keyArgs[1]),
            bucketId: Number(value.bucket_id),
            provider: value.provider,
            challenger: value.challenger,
            leafIndex: Number(value.target.leaf_index),
            chunkIndex: Number(value.target.chunk_index),
            deposit: value.deposit,
            authorized: value.authorized ?? false,
          }))
          .sort((a, b) => a.deadline - b.deadline)
      }),

      safe('challenge stats', { totalIssued: 0, upheld: 0, dismissed: 0 } as ChallengeAggregates, async () => {
        const entries = await api.query.StorageProvider.ChallengerStats.getEntries()
        const agg: ChallengeAggregates = { totalIssued: 0, upheld: 0, dismissed: 0 }
        for (const { value } of entries) {
          agg.totalIssued += value.total_challenges
          agg.upheld += value.successful_challenges
          agg.dismissed += value.failed_challenges
        }
        return agg
      }),

      safe('bucket counter', 0, async () =>
        Number(await api.query.StorageProvider.NextBucketId.getValue())
      ),
    ])

  return {
    providers,
    agreements,
    buckets,
    openChallenges,
    challengeAggregates,
    bucketsEverCreated,
    failedSections,
    fetchedAt: Date.now(),
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pure derivations (anchor-clock aware, computed at render time)
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Agreement status against the pallet's anchor clock. Expired rows persist in
 * storage until lazily swept, so this derivation is what "active" means.
 * An anchor of 0 (not yet refreshed) marks nothing expired — the safe
 * direction; it self-corrects on the first finalized block.
 *
 * `extensions_blocked` is deliberately NOT a status: on chain it only means
 * "the provider won't renew" and is settable exclusively on live agreements —
 * the agreement stays active until `expires_at`.
 */
export function agreementStatus(a: AgreementRow, anchorBlock: number): AgreementStatus {
  if (anchorBlock > a.expiresAt && a.expiresAt > 0) return 'expired'
  return 'active'
}

/**
 * A provider's 0-100 reputation: the share of resolved challenges it
 * defended. Mirrors the pallet's `reputation_score` (runtime_api.rs) over the
 * same stats fields, so no extra RPC is needed. Providers with no resolved
 * challenges score 100 — benefit of the doubt, matching the chain.
 */
export function reputationScore(stats: ProviderStats): number {
  const defended = stats.challengesDefendedAuthorized + stats.challengesDefendedPublic
  const total = defended + stats.challengesFailed
  if (total === 0) return 100
  return Math.min(Math.floor((defended * 100) / total), 100)
}

export interface SummaryStats {
  providerCount: number
  totalStake: bigint
  /** Σ committed_bytes over providers — bytes currently under agreement. */
  totalData: bigint
  activeAgreements: number
  bucketCount: number
  openChallenges: number
}

export function summarize(s: NetworkSnapshot, anchorBlock: number): SummaryStats {
  return {
    providerCount: s.providers.length,
    totalStake: s.providers.reduce((acc, p) => acc + p.stake, 0n),
    totalData: s.providers.reduce((acc, p) => acc + p.committedBytes, 0n),
    activeAgreements: s.agreements.filter((a) => agreementStatus(a, anchorBlock) === 'active')
      .length,
    bucketCount: s.buckets.length,
    openChallenges: s.openChallenges.length,
  }
}

/**
 * Committed quota per bucket: Σ max_bytes over its agreements. Buckets carry
 * no byte size on chain, so this is the honest "size" figure. Agreements with
 * extensions blocked still count — they are live until expiry.
 */
export function bucketQuotas(agreements: AgreementRow[]): Map<number, bigint> {
  const quotas = new Map<number, bigint>()
  for (const a of agreements) {
    quotas.set(a.bucketId, (quotas.get(a.bucketId) ?? 0n) + a.maxBytes)
  }
  return quotas
}
