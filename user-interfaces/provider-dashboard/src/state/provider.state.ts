/**
 * Provider State - Real chain data queries
 *
 * Queries provider information, settings, agreements, checkpoints,
 * challenges, and earnings from the blockchain.
 */

import { BehaviorSubject, map } from 'rxjs'
import { bind } from '@react-rxjs/core'
import {
  getProviderInfo,
  getProviderSettings,
  getProviderAgreements,
  getProviderCheckpoints,
  getProviderChallenges,
  OnChainProviderInfo,
  OnChainProviderSettings,
  OnChainAgreement,
  OnChainCheckpoint,
  OnChainChallenge,
} from '@/lib/chain-client'

// Types (matching chain types with some UI additions)
export interface ProviderInfo {
  account: string
  stake: bigint
  capacity: bigint
  usedCapacity: bigint
  bucketCount: number
  registeredAt: number
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
  createdAt: number
  deadline: number
}

export interface EarningsSummary {
  totalEarned: bigint
  pendingPayouts: bigint
  lastPayoutBlock: number
  activeAgreementValue: bigint
}

// State subjects
const providerInfo$ = new BehaviorSubject<ProviderInfo | null>(null)
const providerSettings$ = new BehaviorSubject<ProviderSettings | null>(null)
const agreements$ = new BehaviorSubject<Agreement[]>([])
const checkpoints$ = new BehaviorSubject<Checkpoint[]>([])
const challenges$ = new BehaviorSubject<Challenge[]>([])
const earnings$ = new BehaviorSubject<EarningsSummary | null>(null)
const isLoading$ = new BehaviorSubject<boolean>(false)
const error$ = new BehaviorSubject<string | null>(null)

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
export const [useAgreements] = bind(agreements$, [])
export const [useActiveAgreements] = bind(activeAgreements$, [])
export const [useCheckpoints] = bind(checkpoints$, [])
export const [useChallenges] = bind(challenges$, [])
export const [usePendingChallenges] = bind(pendingChallenges$, [])
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
    // Query provider info and settings in parallel
    const [chainInfo, chainSettings] = await Promise.all([
      getProviderInfo(address),
      getProviderSettings(address),
    ])

    if (chainInfo) {
      providerInfo$.next(convertProviderInfo(address, chainInfo))
    } else {
      providerInfo$.next(null)
    }

    if (chainSettings) {
      providerSettings$.next(convertProviderSettings(chainSettings))
    } else {
      providerSettings$.next(null)
    }

    // Query agreements, checkpoints, challenges in parallel
    const [chainAgreements, chainCheckpoints, chainChallenges] = await Promise.all([
      getProviderAgreements(address),
      getProviderCheckpoints(address),
      getProviderChallenges(address),
    ])

    agreements$.next(chainAgreements.map(convertAgreement))
    checkpoints$.next(chainCheckpoints.map(convertCheckpoint))
    challenges$.next(chainChallenges.map(convertChallenge))

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
  signer: any, // InjectedPolkadotAccount
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
  signer: any, // InjectedPolkadotAccount
  onProgress?: (status: import('@/lib/chain-client').TxStatus) => void
): Promise<void> {
  const current = providerSettings$.getValue()
  if (!current) throw new Error('No provider settings to update')

  isLoading$.next(true)
  error$.next(null)

  try {
    const { submitUpdateSettings } = await import('@/lib/chain-client')
    const fullSettings = { ...current, ...settings }
    await submitUpdateSettings(fullSettings, signer, onProgress)

    // Update local state optimistically
    providerSettings$.next(fullSettings)
  } catch (err) {
    error$.next(err instanceof Error ? err.message : 'Update failed')
    throw err
  } finally {
    isLoading$.next(false)
  }
}

/**
 * Add stake (submits extrinsic via wallet)
 */
export async function addStake(amount: bigint, signer: any): Promise<void> {
  isLoading$.next(true)
  error$.next(null)

  try {
    const { submitAddStake } = await import('@/lib/chain-client')
    await submitAddStake(amount, signer)

    // Update local state optimistically
    const current = providerInfo$.getValue()
    if (current) {
      providerInfo$.next({
        ...current,
        stake: current.stake + amount,
      })
    }
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
  challengeId: number,
  proof: Uint8Array,
  signer: any
): Promise<void> {
  isLoading$.next(true)
  error$.next(null)

  try {
    const { submitChallengeResponse } = await import('@/lib/chain-client')
    await submitChallengeResponse(challengeId, proof, signer)

    // Update challenge status locally
    const currentChallenges = challenges$.getValue()
    challenges$.next(
      currentChallenges.map((c) =>
        c.id === challengeId ? { ...c, status: 'responded' as const } : c
      )
    )
  } catch (err) {
    error$.next(err instanceof Error ? err.message : 'Challenge response failed')
    throw err
  } finally {
    isLoading$.next(false)
  }
}

export function clearProviderState(): void {
  providerInfo$.next(null)
  providerSettings$.next(null)
  agreements$.next([])
  checkpoints$.next([])
  challenges$.next([])
  earnings$.next(null)
  error$.next(null)
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
    createdAt: chain.createdAt,
    deadline: chain.deadline,
  }
}

// Export actions for testing
export const providerActions = {
  loadProviderData,
  registerProvider,
  updateSettings,
  addStake,
  respondToChallenge,
  clearProviderState,
}
