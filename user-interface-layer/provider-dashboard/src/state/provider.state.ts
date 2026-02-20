import { BehaviorSubject, combineLatest, map } from 'rxjs'
import { bind } from '@react-rxjs/core'

// Types
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

// Actions
export async function loadProviderData(address: string): Promise<void> {
  isLoading$.next(true)
  error$.next(null)

  try {
    // TODO: Query chain for actual provider data
    // For now, simulate with mock data
    await new Promise((resolve) => setTimeout(resolve, 500))

    // Mock provider info
    providerInfo$.next({
      account: address,
      stake: 1_000_000_000_000_000n, // 1000 tokens
      capacity: 1_073_741_824_000n, // 1 TB
      usedCapacity: 536_870_912_000n, // 500 GB
      bucketCount: 5,
      registeredAt: Date.now() - 86400000 * 30, // 30 days ago
    })

    providerSettings$.next({
      minDuration: 100,
      maxDuration: 100_000,
      pricePerByte: 1_000_000n,
      acceptingPrimary: true,
      acceptingReplica: true,
      replicaSyncPrice: 500_000n,
      acceptingExtensions: true,
      maxCapacity: 1_073_741_824_000n,
    })

    // Mock agreements
    agreements$.next([
      {
        id: 1,
        bucketId: 0,
        provider: address,
        user: '5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty',
        maxBytes: 107_374_182_400n, // 100 GB
        pricePerByte: 1_000_000n,
        startBlock: 1000,
        endBlock: 11000,
        isPrimary: true,
        status: 'active',
      },
      {
        id: 2,
        bucketId: 1,
        provider: address,
        user: '5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y',
        maxBytes: 53_687_091_200n, // 50 GB
        pricePerByte: 1_000_000n,
        startBlock: 2000,
        endBlock: 12000,
        isPrimary: true,
        status: 'active',
      },
    ])

    // Mock checkpoints
    checkpoints$.next([
      {
        bucketId: 0,
        mmrRoot: '0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef',
        leafCount: 1000,
        submittedAt: Date.now() - 3600000,
        blockNumber: 5000,
        providers: [address],
      },
    ])

    // Mock earnings
    earnings$.next({
      totalEarned: 50_000_000_000_000n,
      pendingPayouts: 5_000_000_000_000n,
      lastPayoutBlock: 4500,
      activeAgreementValue: 160_000_000_000_000n,
    })
  } catch (err) {
    error$.next(err instanceof Error ? err.message : 'Failed to load provider data')
  } finally {
    isLoading$.next(false)
  }
}

export async function registerProvider(stake: bigint, settings: ProviderSettings): Promise<void> {
  isLoading$.next(true)
  error$.next(null)

  try {
    // TODO: Submit registration extrinsic
    await new Promise((resolve) => setTimeout(resolve, 1000))

    // Reload data after registration
    // loadProviderData would be called with the current account
  } catch (err) {
    error$.next(err instanceof Error ? err.message : 'Registration failed')
    throw err
  } finally {
    isLoading$.next(false)
  }
}

export async function updateSettings(settings: Partial<ProviderSettings>): Promise<void> {
  const current = providerSettings$.getValue()
  if (!current) throw new Error('No provider settings to update')

  isLoading$.next(true)
  error$.next(null)

  try {
    // TODO: Submit update extrinsic
    await new Promise((resolve) => setTimeout(resolve, 500))

    providerSettings$.next({ ...current, ...settings })
  } catch (err) {
    error$.next(err instanceof Error ? err.message : 'Update failed')
    throw err
  } finally {
    isLoading$.next(false)
  }
}

export async function respondToChallenge(
  challengeId: number,
  proof: Uint8Array
): Promise<void> {
  isLoading$.next(true)
  error$.next(null)

  try {
    // TODO: Submit challenge response extrinsic
    await new Promise((resolve) => setTimeout(resolve, 500))

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

// Export actions for testing
export const providerActions = {
  loadProviderData,
  registerProvider,
  updateSettings,
  respondToChallenge,
  clearProviderState,
}
