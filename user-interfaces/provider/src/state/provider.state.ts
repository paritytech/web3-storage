/**
 * Provider State - Real chain data queries
 *
 * Queries provider information, settings, agreements, checkpoints,
 * challenges, and earnings from the blockchain.
 */

import { BehaviorSubject, map } from 'rxjs'
import { bind } from '@react-rxjs/core'
import {
  getProviderData,
  getProviderAgreements,
  getAgreementRequests,
  getProviderCheckpoints,
  getProviderChallenges,
  getBucketDetails,
  OnChainProviderInfo,
  OnChainProviderSettings,
  OnChainAgreement,
  OnChainAgreementRequest,
  OnChainCheckpoint,
  OnChainChallenge,
  OnChainBucketDetails,
} from '@/lib/chain-client'
import { isSameAddress } from '@web3-storage/sdk'
import { getCurrentBlock } from '@/state/chain.state'
import { getProviderHttp } from '@/state/network.state'

// Types (matching chain types with some UI additions)
export interface ProviderInfo {
  account: string
  stake: bigint
  capacity: bigint
  usedCapacity: bigint
  bucketCount: number
  registeredAt: number
  multiaddr?: string
}

export interface ProviderSettings {
  minDuration: number
  maxDuration: number
  pricePerByte: bigint
  acceptingPrimary: boolean
  acceptingReplica: boolean
  replicaSyncPrice: bigint | null
  acceptingExtensions: boolean
  maxCapacity: bigint
}

export interface Agreement {
  id: number
  bucketId: number
  provider: string
  user: string
  maxBytes: bigint
  pricePerByte: bigint
  startBlock: number
  endBlock: number
  isPrimary: boolean
  status: 'active' | 'expired' | 'terminated'
}

export interface Checkpoint {
  bucketId: number
  mmrRoot: string
  leafCount: number
  submittedAt: number
  blockNumber: number
  providers: string[]
}

export interface Challenge {
  id: number
  bucketId: number
  challenger: string
  provider: string
  leafIndex: number
  status: 'pending' | 'responded' | 'slashed' | 'expired'
  challengeType?: 'offchain' | 'checkpoint' | 'unknown'
  createdAt: number
  deadline: number
}

export interface AgreementRequest {
  bucketId: number
  requester: string
  maxBytes: bigint
  paymentLocked: bigint
  duration: number
  expiresAt: number
}

export interface EarningsSummary {
  totalEarned: bigint
  pendingPayouts: bigint
  lastPayoutBlock: number
  activeAgreementValue: bigint
}

export interface BucketMember {
  account: string
  role: 'Admin' | 'Writer' | 'Reader'
}

export interface BucketDetail {
  bucketId: number
  members: BucketMember[]
  frozenStartSeq: number | null
  minProviders: number
  primaryProviders: string[]
  totalSnapshots: number
  snapshot: { mmrRoot: string; startSeq: number; leafCount: number; checkpointBlock: number } | null
  historicalRoots: Array<{ position: number; root: string }>
  agreement: {
    maxBytes: bigint
    expiresAt: number
    startedAt: number
    isPrimary: boolean
    paymentLocked: bigint
    status: 'active' | 'expired' | 'terminated'
  }
  checkpointConfig: { interval: number; gracePeriod: number; enabled: boolean } | null
  lastCheckpointWindow: bigint | null
  checkpointPoolBalance: bigint
  checkpointReward: bigint
  isCheckpointOverdue: boolean
}

// Persist challenges to localStorage so they survive page reloads.
// Stores genesis hash alongside data to auto-clear on chain restart.
const CHALLENGES_STORAGE_KEY = 'provider-challenges'
const CHALLENGES_GENESIS_KEY = 'provider-challenges-genesis'

function loadPersistedChallenges(): Challenge[] {
  try {
    const raw = localStorage.getItem(CHALLENGES_STORAGE_KEY)
    return raw ? JSON.parse(raw) : []
  } catch { return [] }
}
function persistChallenges(challenges: Challenge[]) {
  try {
    localStorage.setItem(CHALLENGES_STORAGE_KEY, JSON.stringify(challenges))
  } catch { /* ignore */ }
}
/**
 * Clear stale challenge cache if connected to a different chain (genesis hash changed).
 * Called at data load time.
 */
function clearStaleCache(genesisHash: string) {
  const stored = localStorage.getItem(CHALLENGES_GENESIS_KEY)
  if (stored && stored !== genesisHash) {
    localStorage.removeItem(CHALLENGES_STORAGE_KEY)
    challenges$.next([])
  }
  localStorage.setItem(CHALLENGES_GENESIS_KEY, genesisHash)
}

// State subjects
const providerInfo$ = new BehaviorSubject<ProviderInfo | null>(null)
const providerSettings$ = new BehaviorSubject<ProviderSettings | null>(null)
const agreementRequests$ = new BehaviorSubject<AgreementRequest[]>([])
const agreements$ = new BehaviorSubject<Agreement[]>([])
const checkpoints$ = new BehaviorSubject<Checkpoint[]>([])
const challenges$ = new BehaviorSubject<Challenge[]>(loadPersistedChallenges())
const bucketDetails$ = new BehaviorSubject<BucketDetail[]>([])
const earnings$ = new BehaviorSubject<EarningsSummary | null>(null)
const isLoading$ = new BehaviorSubject<boolean>(false)
const error$ = new BehaviorSubject<string | null>(null)

// Persist challenges whenever they change
challenges$.subscribe(persistChallenges)

// Derived state
const isRegistered$ = providerInfo$.pipe(map((info) => info !== null))

const activeAgreements$ = agreements$.pipe(
  map((agreements) => agreements.filter((a) => a.status === 'active'))
)

const pendingChallenges$ = challenges$.pipe(
  map((challenges) => challenges.filter((c) => c.status === 'pending'))
)

const capacityUsage$ = providerInfo$.pipe(
  map((info) => {
    if (!info || info.capacity === 0n) return 0
    return Number((info.usedCapacity * 100n) / info.capacity)
  })
)

// React hooks
export const [useProviderInfo] = bind(providerInfo$, null)
export const [useProviderSettings] = bind(providerSettings$, null)
export const [useAgreementRequests] = bind(agreementRequests$, [])
export const [useAgreements] = bind(agreements$, [])
export const [useActiveAgreements] = bind(activeAgreements$, [])
export const [useCheckpoints] = bind(checkpoints$, [])
export const [useChallenges] = bind(challenges$, [])
export const [usePendingChallenges] = bind(pendingChallenges$, [])
export const [useBucketDetails] = bind(bucketDetails$, [])
export const [useEarnings] = bind(earnings$, null)
export const [useIsRegistered] = bind(isRegistered$, false)
export const [useCapacityUsage] = bind(capacityUsage$, 0)
export const [useIsProviderLoading] = bind(isLoading$, false)
export const [useProviderError] = bind(error$, null)

// ─────────────────────────────────────────────────────────────────────────────
// Actions
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Load provider data from chain
 */
export async function loadProviderData(address: string): Promise<void> {
  isLoading$.next(true)
  error$.next(null)

  try {
    // Clear stale challenge cache if chain restarted (different genesis)
    try {
      const { getGenesisHash } = await import('@/lib/chain-client')
      clearStaleCache(await getGenesisHash())
    } catch { /* ignore */ }

    // Query provider info and settings in a single RPC call
    const providerData = await getProviderData(address)

    if (providerData) {
      providerInfo$.next(convertProviderInfo(address, providerData.info))
      providerSettings$.next(convertProviderSettings(providerData.settings))
    } else {
      providerInfo$.next(null)
      providerSettings$.next(null)
    }

    // Query agreements, requests, checkpoints, challenges in parallel
    const [chainAgreements, chainRequests, chainCheckpoints, chainChallenges] = await Promise.all([
      getProviderAgreements(address),
      getAgreementRequests(address),
      getProviderCheckpoints(address),
      getProviderChallenges(address),
    ])

    agreements$.next(chainAgreements.map(convertAgreement).sort((a, b) => a.bucketId - b.bucketId))
    agreementRequests$.next(chainRequests.map(convertAgreementRequest))
    checkpoints$.next(chainCheckpoints.map(convertCheckpoint))
    // Merge new challenges with existing cache — don't discard responded ones
    // that are no longer in storage or recent events.
    const newChallenges = chainChallenges.map(convertChallenge)
    const existing = challenges$.getValue()
    const merged = new Map<string, Challenge>()
    // Keep existing (may include responded/slashed from prior polls)
    for (const c of existing) merged.set(`${c.deadline}-${c.id}`, c)
    // Overlay with fresh data (updates status for ones still visible)
    for (const c of newChallenges) merged.set(`${c.deadline}-${c.id}`, c)
    challenges$.next(Array.from(merged.values()).sort((a, b) => b.deadline - a.deadline))

    // Phase 2: Fetch bucket details (depends on agreement data for bucket IDs)
    const bucketIds = [...new Set(
      chainAgreements
        .filter((a) => isSameAddress(a.provider, address))
        .map((a) => a.bucketId)
    )]
    if (bucketIds.length > 0) {
      try {
        const chainBucketDetails = await getBucketDetails(bucketIds, address)
        const currentBlock = getCurrentBlock() || 0
        const agreementsByBucket = new Map<number, typeof chainAgreements[0]>()
        for (const a of chainAgreements) {
          if (isSameAddress(a.provider, address)) agreementsByBucket.set(a.bucketId, a)
        }
        bucketDetails$.next(
          chainBucketDetails.map((bd) => {
            const agr = agreementsByBucket.get(bd.bucketId)
            let isOverdue = false
            if (bd.checkpointConfig?.enabled && bd.lastCheckpointWindow != null && currentBlock > 0) {
              const expectedWindow = BigInt(Math.floor(currentBlock / bd.checkpointConfig.interval))
              isOverdue = expectedWindow > bd.lastCheckpointWindow + 1n
            }
            return convertBucketDetail(bd, agr || null, isOverdue)
          }).sort((a, b) => a.bucketId - b.bucketId)
        )
      } catch (e) {
        console.warn('Failed to load bucket details:', e)
      }
    }

    // Fetch real storage usage from provider node /stats endpoint
    try {
      const providerHttp = getProviderHttp()
      const statsRes = await fetch(`${providerHttp}/stats`)
      if (statsRes.ok) {
        const stats = await statsRes.json()
        if (providerData) {
          const info = convertProviderInfo(address, providerData.info)
          info.usedCapacity = BigInt(stats.total_bytes || 0)
          providerInfo$.next(info)
        }
      }
    } catch {
      // Provider node may not be reachable — ignore
    }

    // Calculate earnings from agreements
    const activeAgreements = chainAgreements.filter((a) => a.status === 'active')
    const activeValue = activeAgreements.reduce(
      (sum, a) => sum + a.maxBytes * a.pricePerByte * BigInt(a.endBlock - a.startBlock),
      0n
    )

    earnings$.next({
      totalEarned: 0n, // Would need historical data
      pendingPayouts: 0n, // Would need escrow queries
      lastPayoutBlock: 0,
      activeAgreementValue: activeValue,
    })
  } catch (err) {
    error$.next(err instanceof Error ? err.message : 'Failed to load provider data')
  } finally {
    isLoading$.next(false)
  }
}

// Re-export TxStatus for UI components
export type { TxStatus, TxProgressCallback } from '@/lib/chain-client'

/**
 * Register as a provider (submits extrinsic via wallet)
 */
export async function registerProvider(
  stake: bigint,
  multiaddr: string,
  settings: ProviderSettings,
  signer: import('polkadot-api/pjs-signer').InjectedPolkadotAccount,
  onProgress?: (status: import('@/lib/chain-client').TxStatus) => void
): Promise<void> {
  isLoading$.next(true)
  error$.next(null)

  try {
    // Import submitRegisterProvider from chain-client
    const { submitRegisterProvider } = await import('@/lib/chain-client')
    await submitRegisterProvider({ stake, multiaddr }, settings, signer, onProgress)

    // Data will be reloaded after successful registration
  } catch (err) {
    error$.next(err instanceof Error ? err.message : 'Registration failed')
    throw err
  } finally {
    isLoading$.next(false)
  }
}

/**
 * Update provider settings (submits extrinsic via wallet)
 */
export async function updateSettings(
  settings: Partial<ProviderSettings>,
  signer: import('polkadot-api/pjs-signer').InjectedPolkadotAccount,
  onProgress?: (status: import('@/lib/chain-client').TxStatus) => void
): Promise<void> {
  if (isLoading$.getValue()) throw new Error('Another operation is in progress')

  const current = providerSettings$.getValue()
  if (!current) throw new Error('No provider settings to update')

  isLoading$.next(true)
  error$.next(null)

  try {
    const { submitUpdateSettings } = await import('@/lib/chain-client')
    const fullSettings = { ...current, ...settings }
    await submitUpdateSettings(fullSettings, signer, onProgress)

    // Reload from chain to get confirmed state (not just optimistic)
    const address = providerInfo$.getValue()?.account
    if (address) {
      await loadProviderData(address)
    } else {
      providerSettings$.next(fullSettings)
    }
  } catch (err) {
    error$.next(err instanceof Error ? err.message : 'Update failed')
    throw err
  } finally {
    isLoading$.next(false)
  }
}

/**
 * Update provider multiaddr (submits extrinsic via wallet)
 */
export async function updateMultiaddr(
  multiaddr: string,
  signer: import('polkadot-api/pjs-signer').InjectedPolkadotAccount,
  onProgress?: (status: import('@/lib/chain-client').TxStatus) => void
): Promise<void> {
  if (isLoading$.getValue()) throw new Error('Another operation is in progress')
  isLoading$.next(true)
  error$.next(null)

  try {
    const { submitUpdateMultiaddr } = await import('@/lib/chain-client')
    await submitUpdateMultiaddr(multiaddr, signer, onProgress)

    // Reload from chain
    const address = providerInfo$.getValue()?.account
    if (address) await loadProviderData(address)
  } catch (err) {
    error$.next(err instanceof Error ? err.message : 'Update multiaddr failed')
    throw err
  } finally {
    isLoading$.next(false)
  }
}

/**
 * Add stake (submits extrinsic via wallet)
 */
export async function addStake(
  amount: bigint,
  signer: import('polkadot-api/pjs-signer').InjectedPolkadotAccount
): Promise<void> {
  if (isLoading$.getValue()) throw new Error('Another operation is in progress')
  isLoading$.next(true)
  error$.next(null)

  try {
    const { submitAddStake } = await import('@/lib/chain-client')
    await submitAddStake(amount, signer)

    // Reload from chain
    const address = providerInfo$.getValue()?.account
    if (address) await loadProviderData(address)
  } catch (err) {
    error$.next(err instanceof Error ? err.message : 'Add stake failed')
    throw err
  } finally {
    isLoading$.next(false)
  }
}

/**
 * Respond to a challenge (submits extrinsic via wallet)
 */
export async function respondToChallenge(
  _challengeId: number,
  _proof: Uint8Array,
  _signer: import('polkadot-api/pjs-signer').InjectedPolkadotAccount
): Promise<void> {
  // The runtime extrinsic takes a structured `ChallengeResponse` payload and
  // a `ChallengeId { deadline, index }`, not just bytes + a number. The
  // submission helper was removed during the PAPI v2 migration; add it back
  // when implementing the manual challenge-response UI flow.
  throw new Error('respondToChallenge: not implemented in current build')
}

export function clearProviderState(): void {
  providerInfo$.next(null)
  providerSettings$.next(null)
  agreementRequests$.next([])
  agreements$.next([])
  checkpoints$.next([])
  challenges$.next([])
  bucketDetails$.next([])
  earnings$.next(null)
  error$.next(null)
}

/**
 * Add or update a challenge from a real-time event subscription.
 * Merges with existing challenges and persists to localStorage.
 */
export function addChallengeFromEvent(onChainChallenge: import('@/lib/chain-client').OnChainChallenge): void {
  const challenge = convertChallenge(onChainChallenge)
  const key = `${challenge.deadline}-${challenge.id}`
  const existing = challenges$.getValue()
  const merged = new Map<string, Challenge>()
  for (const c of existing) merged.set(`${c.deadline}-${c.id}`, c)

  const prev = merged.get(key)
  if (prev) {
    // Update status but keep richer data from ChallengeCreated
    merged.set(key, { ...prev, status: challenge.status || prev.status })
  } else {
    merged.set(key, challenge)
  }

  challenges$.next(Array.from(merged.values()).sort((a, b) => b.deadline - a.deadline))
}

// ─────────────────────────────────────────────────────────────────────────────
// Converters (chain types to UI types)
// ─────────────────────────────────────────────────────────────────────────────

function convertProviderInfo(address: string, chain: OnChainProviderInfo): ProviderInfo {
  return {
    account: address,
    stake: chain.stake,
    capacity: chain.capacity ?? 0n,
    usedCapacity: chain.usedCapacity ?? 0n,
    bucketCount: chain.activeBuckets,
    registeredAt: chain.registeredAt,
    multiaddr: chain.multiaddr,
  }
}

function convertProviderSettings(chain: OnChainProviderSettings): ProviderSettings {
  return {
    minDuration: chain.minDuration,
    maxDuration: chain.maxDuration,
    pricePerByte: chain.pricePerByte,
    acceptingPrimary: chain.acceptingPrimary,
    acceptingReplica: chain.acceptingReplica,
    replicaSyncPrice: chain.replicaSyncPrice,
    acceptingExtensions: chain.acceptingExtensions,
    maxCapacity: chain.maxCapacity,
  }
}

function convertAgreementRequest(chain: OnChainAgreementRequest): AgreementRequest {
  return {
    bucketId: chain.bucketId,
    requester: chain.requester,
    maxBytes: chain.maxBytes,
    paymentLocked: chain.paymentLocked,
    duration: chain.duration,
    expiresAt: chain.expiresAt,
  }
}

function convertAgreement(chain: OnChainAgreement): Agreement {
  return {
    id: chain.id,
    bucketId: chain.bucketId,
    provider: chain.provider,
    user: chain.user,
    maxBytes: chain.maxBytes,
    pricePerByte: chain.pricePerByte,
    startBlock: chain.startBlock,
    endBlock: chain.endBlock,
    isPrimary: chain.isPrimary,
    status: chain.status,
  }
}

function convertCheckpoint(chain: OnChainCheckpoint): Checkpoint {
  return {
    bucketId: chain.bucketId,
    mmrRoot: chain.mmrRoot,
    leafCount: chain.leafCount,
    submittedAt: chain.submittedAt,
    blockNumber: chain.blockNumber,
    providers: chain.providers,
  }
}

function convertChallenge(chain: OnChainChallenge): Challenge {
  return {
    id: chain.id,
    bucketId: chain.bucketId,
    challenger: chain.challenger,
    provider: chain.provider,
    leafIndex: chain.leafIndex,
    status: chain.status,
    challengeType: chain.challengeType,
    createdAt: chain.createdAt,
    deadline: chain.deadline,
  }
}

function convertBucketDetail(
  chain: OnChainBucketDetails,
  agreement: { bucketId: number; maxBytes: bigint; endBlock: number; startBlock: number; isPrimary: boolean; paymentLocked: bigint; status: 'active' | 'expired' | 'terminated' } | null,
  isCheckpointOverdue: boolean
): BucketDetail {
  return {
    bucketId: chain.bucketId,
    members: chain.members,
    frozenStartSeq: chain.frozenStartSeq,
    minProviders: chain.minProviders,
    primaryProviders: chain.primaryProviders,
    totalSnapshots: chain.totalSnapshots,
    snapshot: chain.snapshot,
    historicalRoots: chain.historicalRoots,
    agreement: agreement ? {
      maxBytes: agreement.maxBytes,
      expiresAt: agreement.endBlock,
      startedAt: agreement.startBlock,
      isPrimary: agreement.isPrimary,
      paymentLocked: agreement.paymentLocked,
      status: agreement.status,
    } : {
      maxBytes: 0n, expiresAt: 0, startedAt: 0, isPrimary: false, paymentLocked: 0n, status: 'expired' as const,
    },
    checkpointConfig: chain.checkpointConfig,
    lastCheckpointWindow: chain.lastCheckpointWindow,
    checkpointPoolBalance: chain.checkpointPoolBalance,
    checkpointReward: chain.checkpointReward,
    isCheckpointOverdue,
  }
}

// Export actions for testing
export const providerActions = {
  loadProviderData,
  registerProvider,
  updateSettings,
  updateMultiaddr,
  addStake,
  respondToChallenge,
  clearProviderState,
}
