// SPDX-License-Identifier: GPL-3.0-only

/**
 * Explorer data layer — one-shot network-wide snapshots of on-chain state.
 *
 * Most sections are read via full-map `getEntries()` scans, matching how the
 * other apps read chain state. Fine at current network sizes; the pallet's
 * paginated runtime APIs are the upgrade path when scans get expensive.
 *
 * Providers already went that way: `reputation` and `available_capacity` are
 * computed by the pallet, never stored, so only the runtime API has them.
 */

import { BlockNotPinnedError } from 'polkadot-api'
import { requireApi, requireClient } from '@/lib/chain-client'

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
  /** Every payment this registration has received; a slash never reduces it. */
  lifetimeRevenue: bigint
  /** 0-100, computed on-chain by `ProviderStats::reputation`. */
  reputation: number
}

export interface ProviderRow {
  address: string
  multiaddr: string
  stake: bigint
  /** Bytes currently under agreement (the pallet keeps this in sync). */
  committedBytes: bigint
  /** Free capacity per the chain; `undefined` = unlimited, not `0n` ("full"). */
  availableCapacity: bigint | undefined
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
 * The independently loaded parts of a snapshot. One name per `safe()` call in
 * `loadNetworkSnapshotAt`; `summarize` and the pages test against it, so a
 * rename is a compile error, not a stat that silently reports 0 again.
 */
export type Section = 'providers' | 'agreements' | 'buckets' | 'challenges' | 'bucket counter'

export interface NetworkSnapshot {
  providers: ProviderRow[]
  agreements: AgreementRow[]
  buckets: BucketRow[]
  openChallenges: ChallengeRow[]
  /** NextBucketId — buckets ever created (deleted ones included). */
  bucketsEverCreated: number
  /** Sections whose query failed (e.g. storage item missing on an older runtime). */
  failedSections: Section[]
  fetchedAt: number
}

// ─────────────────────────────────────────────────────────────────────────────
// Snapshot loading
// ─────────────────────────────────────────────────────────────────────────────

/** Providers per `StorageProviderApi.providers` call. */
const PROVIDER_PAGE_SIZE = 200

/** Block and abort signal shared by every read of one snapshot. */
interface ReadOptions {
  at: string
  signal: AbortSignal
}

/**
 * Walk every registered provider through the paginated runtime API, every
 * page at the same block. The pallet pages with
 * `Providers::iter().skip(offset)` over hash-ordered keys: a key inserted
 * before the page boundary between two calls moves the boundary element onto
 * the next page too (one provider repeated); a key removed before it moves
 * the next element across the boundary (one provider skipped).
 */
async function fetchProviders(opts: ReadOptions): Promise<ProviderRow[]> {
  const api = requireApi()
  const rows: ProviderRow[] = []

  for (let offset = 0; ; offset += PROVIDER_PAGE_SIZE) {
    const page = await api.apis.StorageProviderApi.providers(offset, PROVIDER_PAGE_SIZE, opts)

    for (const [account, info] of page) {
      rows.push({
        address: account,
        multiaddr: new TextDecoder().decode(info.multiaddr),
        stake: info.stake,
        committedBytes: BigInt(info.committed_bytes),
        availableCapacity:
          info.available_capacity == null ? undefined : BigInt(info.available_capacity),
        settings: {
          minDuration: info.min_duration,
          maxDuration: info.max_duration,
          pricePerByte: info.price_per_byte,
          acceptingPrimary: info.accepting_primary,
          replicaSyncPrice: info.replica_sync_price ?? undefined,
          acceptingExtensions: info.accepting_extensions,
          maxCapacity: BigInt(info.max_capacity),
        },
        stats: {
          registeredAt: info.stats.registered_at,
          agreementsTotal: info.stats.agreements_total,
          agreementsExtended: info.stats.agreements_extended,
          agreementsNotExtended: info.stats.agreements_not_extended,
          agreementsBurned: info.stats.agreements_burned,
          totalBytesCommitted: BigInt(info.stats.total_bytes_committed),
          challengesDefendedAuthorized: info.stats.challenges_received_authorized,
          challengesDefendedPublic: info.stats.challenges_received_public,
          challengesFailed: info.stats.challenges_failed,
          lifetimeRevenue: info.stats.lifetime_revenue,
          reputation: info.stats.reputation,
        },
        deregisterAt: info.deregister_at ?? undefined,
      })
    }

    // A short page is the last one — the API caps at `limit` per call.
    if (page.length < PROVIDER_PAGE_SIZE) break
  }

  return rows
}

/**
 * Retries of a whole snapshot after PAPI unpinned the block it was reading.
 *
 * PAPI unpins a finalized block as soon as no read holds it and a newer block
 * is finalized. The exposed windows are the gap between `getFinalizedBlock()`
 * and the first read, and the gaps between provider pages — each a few
 * milliseconds against a block time of seconds, so two retries are ample.
 */
const MAX_UNPINNED_RETRIES = 2

/**
 * Load every section at one finalized block, so an agreement in the snapshot
 * never references a provider the same snapshot does not contain.
 *
 * A read at a hash PAPI has unpinned fails with `BlockNotPinnedError` unless
 * the node serves `archive_*`. That is not a section failure: the load
 * restarts at the current finalized block.
 */
export async function loadNetworkSnapshot(): Promise<NetworkSnapshot> {
  for (let attempt = 0; ; attempt++) {
    const at = (await requireClient().getFinalizedBlock()).hash
    try {
      return await loadNetworkSnapshotAt(at)
    } catch (e) {
      if (!(e instanceof BlockNotPinnedError) || attempt >= MAX_UNPINNED_RETRIES) throw e
    }
  }
}

async function loadNetworkSnapshotAt(at: string): Promise<NetworkSnapshot> {
  const api = requireApi()
  const failedSections: Section[] = []
  // One block for every read; aborted as a whole once any read finds the
  // block unpinned, so the discarded snapshot stops loading.
  const abort = new AbortController()
  const opts = { at, signal: abort.signal }

  // A query failing (most likely a storage item missing on an older live
  // runtime) degrades its own section instead of blanking the whole app.
  async function safe<T>(section: Section, fallback: T, run: () => Promise<T>): Promise<T> {
    try {
      return await run()
    } catch (e) {
      // The block is gone for every section, not just this one; the caller
      // restarts the whole snapshot.
      if (e instanceof BlockNotPinnedError) {
        abort.abort()
        throw e
      }
      // This snapshot is already discarded; the error that aborted it wins.
      if (abort.signal.aborted) throw e
      console.warn(`explorer: failed to load ${section}:`, e)
      failedSections.push(section)
      return fallback
    }
  }

  const [providers, agreements, buckets, openChallenges, bucketsEverCreated] = await Promise.all([
      safe('providers', [] as ProviderRow[], () => fetchProviders(opts)),

      safe('agreements', [] as AgreementRow[], async () => {
        const entries = await api.query.StorageProvider.StorageAgreements.getEntries(opts)
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
        const entries = await api.query.StorageProvider.Buckets.getEntries(opts)
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
        const entries = await api.query.StorageProvider.Challenges.getEntries(opts)
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

      safe('bucket counter', 0, async () =>
        Number(await api.query.StorageProvider.NextBucketId.getValue(opts))
      ),
    ])

  return {
    providers,
    agreements,
    buckets,
    openChallenges,
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
 * Network totals. A field is `undefined` when the section it derives from
 * failed to load — a failed section must not report 0, which reads as a real
 * measurement ("this network has no providers").
 */
export interface SummaryStats {
  providerCount: number | undefined
  totalStake: bigint | undefined
  /** Σ committed_bytes over providers — bytes currently under agreement. */
  totalData: bigint | undefined
  activeAgreements: number | undefined
  bucketCount: number | undefined
  /** NextBucketId — buckets ever created, deleted ones included. */
  bucketsEverCreated: number | undefined
  openChallenges: number | undefined
}

export function summarize(s: NetworkSnapshot, anchorBlock: number): SummaryStats {
  const loaded = (section: Section) => !s.failedSections.includes(section)
  const providers = loaded('providers')

  return {
    providerCount: providers ? s.providers.length : undefined,
    totalStake: providers ? s.providers.reduce((acc, p) => acc + p.stake, 0n) : undefined,
    totalData: providers ? s.providers.reduce((acc, p) => acc + p.committedBytes, 0n) : undefined,
    activeAgreements: loaded('agreements')
      ? s.agreements.filter((a) => agreementStatus(a, anchorBlock) === 'active').length
      : undefined,
    bucketCount: loaded('buckets') ? s.buckets.length : undefined,
    bucketsEverCreated: loaded('bucket counter') ? s.bucketsEverCreated : undefined,
    openChallenges: loaded('challenges') ? s.openChallenges.length : undefined,
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
