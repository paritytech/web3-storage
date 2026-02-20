/**
 * Chain Client - Direct blockchain interaction via WebSocket
 *
 * This module provides direct chain access for:
 * - New providers registering (no provider node needed)
 * - Querying on-chain provider state
 * - Submitting extrinsics (register, updateSettings, etc.)
 */

import { createClient, PolkadotClient } from 'polkadot-api'
import { getWsProvider } from 'polkadot-api/ws-provider/web'
import { BehaviorSubject } from 'rxjs'

// Re-export types
export type { PolkadotClient }

// Chain client singleton
let client: PolkadotClient | null = null
let wsProvider: ReturnType<typeof getWsProvider> | null = null
let unsafeApi: any = null

// Connection state
export const clientReady$ = new BehaviorSubject<boolean>(false)

/**
 * Connect to the blockchain via WebSocket
 */
export async function connectToChain(endpoint: string): Promise<PolkadotClient> {
  if (client) {
    return client
  }

  try {
    wsProvider = getWsProvider(endpoint)
    client = createClient(wsProvider)

    // Get unsafe API for dynamic queries (no type descriptors needed)
    unsafeApi = client.getUnsafeApi()

    clientReady$.next(true)
    return client
  } catch (error) {
    clientReady$.next(false)
    throw error
  }
}

/**
 * Disconnect from the blockchain
 */
export function disconnectFromChain(): void {
  if (client) {
    client.destroy()
    client = null
  }
  wsProvider = null
  unsafeApi = null
  clientReady$.next(false)
}

/**
 * Get the current client instance
 */
export function getClient(): PolkadotClient | null {
  return client
}

/**
 * Get the unsafe API for dynamic queries
 */
export function getUnsafeApi(): any {
  return unsafeApi
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider Pallet Queries
// ─────────────────────────────────────────────────────────────────────────────

export interface OnChainProviderInfo {
  stake: bigint
  activeBuckets: number
  registeredAt: number
}

export interface OnChainProviderSettings {
  minDuration: number
  maxDuration: number
  pricePerByte: bigint
  acceptingPrimary: boolean
  acceptingReplica: boolean
  replicaSyncPrice: bigint | null
  acceptingExtensions: boolean
  maxCapacity: bigint
}

/**
 * Check if an account is registered as a provider
 */
export async function isProviderRegistered(address: string): Promise<boolean> {
  if (!client || !unsafeApi) throw new Error('Not connected to chain')

  try {
    // Query StorageProvider.Providers storage using unsafe API
    const provider = await unsafeApi.query.StorageProvider.Providers.getValue(address)
    console.log('Provider query result for', address, ':', provider)
    return provider !== undefined && provider !== null
  } catch (error) {
    console.error('Error checking provider registration:', error)
    // Fallback to mock for development
    const knownProviders = [
      '5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY', // Alice
    ]
    return knownProviders.includes(address)
  }
}

/**
 * Get provider info from chain
 */
export async function getProviderInfo(address: string): Promise<OnChainProviderInfo | null> {
  if (!client || !unsafeApi) throw new Error('Not connected to chain')

  try {
    const provider = await unsafeApi.query.StorageProvider.Providers.getValue(address)
    if (!provider) return null

    console.log('Provider info:', provider)

    return {
      stake: BigInt(provider.stake?.toString() || '0'),
      activeBuckets: provider.active_buckets || 0,
      registeredAt: provider.registered_at || 0,
    }
  } catch (error) {
    console.error('Error fetching provider info:', error)
    return null
  }
}

/**
 * Get provider settings from chain
 */
export async function getProviderSettings(address: string): Promise<OnChainProviderSettings | null> {
  if (!client || !unsafeApi) throw new Error('Not connected to chain')

  try {
    const settings = await unsafeApi.query.StorageProvider.ProviderSettings.getValue(address)
    if (!settings) return null

    console.log('Provider settings:', settings)

    return {
      minDuration: settings.min_duration || 0,
      maxDuration: settings.max_duration || 0,
      pricePerByte: BigInt(settings.price_per_byte?.toString() || '0'),
      acceptingPrimary: settings.accepting_primary ?? false,
      acceptingReplica: settings.accepting_replica ?? false,
      replicaSyncPrice: settings.replica_sync_price ? BigInt(settings.replica_sync_price.toString()) : null,
      acceptingExtensions: settings.accepting_extensions ?? false,
      maxCapacity: BigInt(settings.max_capacity?.toString() || '0'),
    }
  } catch (error) {
    console.error('Error fetching provider settings:', error)
    return null
  }
}

/**
 * Get account balance
 */
export async function getAccountBalance(address: string): Promise<{
  free: bigint
  reserved: bigint
  frozen: bigint
} | null> {
  if (!client || !unsafeApi) throw new Error('Not connected to chain')

  try {
    const account = await unsafeApi.query.System.Account.getValue(address)
    if (!account) return null

    console.log('Account balance:', account)

    const data = account.data || account
    return {
      free: BigInt(data.free?.toString() || '0'),
      reserved: BigInt(data.reserved?.toString() || '0'),
      frozen: BigInt(data.frozen?.toString() || data.misc_frozen?.toString() || '0'),
    }
  } catch (error) {
    console.error('Error fetching balance:', error)
    // Return mock balance for development
    return {
      free: 10_000_000_000_000_000n,
      reserved: 0n,
      frozen: 0n,
    }
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider Pallet Extrinsics
// ─────────────────────────────────────────────────────────────────────────────

export interface RegisterProviderParams {
  stake: bigint
}

export interface UpdateSettingsParams {
  minDuration: number
  maxDuration: number
  pricePerByte: bigint
  acceptingPrimary: boolean
  acceptingReplica: boolean
  replicaSyncPrice: bigint | null
  acceptingExtensions: boolean
  maxCapacity: bigint
}

/**
 * Build register_provider extrinsic
 *
 * Note: This returns the unsigned extrinsic. The wallet extension
 * will handle signing and submission.
 */
export function buildRegisterProviderTx(params: RegisterProviderParams) {
  if (!client) throw new Error('Not connected to chain')

  // Placeholder - actual implementation:
  // const api = client.getTypedApi(descriptor)
  // return api.tx.StorageProvider.register_provider(params.stake)

  return {
    method: 'StorageProvider.register_provider',
    args: [params.stake.toString()],
  }
}

/**
 * Build update_provider_settings extrinsic
 */
export function buildUpdateSettingsTx(params: UpdateSettingsParams) {
  if (!client) throw new Error('Not connected to chain')

  // Placeholder - actual implementation would use typed API
  return {
    method: 'StorageProvider.update_provider_settings',
    args: [
      params.minDuration,
      params.maxDuration,
      params.pricePerByte.toString(),
      params.acceptingPrimary,
      params.acceptingReplica,
      params.replicaSyncPrice?.toString() ?? null,
      params.acceptingExtensions,
      params.maxCapacity.toString(),
    ],
  }
}

/**
 * Build add_stake extrinsic
 */
export function buildAddStakeTx(amount: bigint) {
  if (!client) throw new Error('Not connected to chain')

  return {
    method: 'StorageProvider.add_stake',
    args: [amount.toString()],
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Block Subscription
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Subscribe to finalized blocks
 */
export function subscribeToBlocks(callback: (blockNumber: number) => void): () => void {
  if (!client) {
    console.warn('Cannot subscribe to blocks: not connected')
    return () => {}
  }

  try {
    // Subscribe to finalized blocks using polkadot-api
    const subscription = client.finalizedBlock$.subscribe({
      next: (block) => {
        callback(block.number)
      },
      error: (err) => {
        console.error('Block subscription error:', err)
      },
    })
    return () => subscription.unsubscribe()
  } catch (error) {
    console.error('Failed to subscribe to blocks:', error)
    // Fallback: mock block increment
    let blockNumber = 1
    const interval = setInterval(() => {
      blockNumber++
      callback(blockNumber)
    }, 6000)
    return () => clearInterval(interval)
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Event Subscription
// ─────────────────────────────────────────────────────────────────────────────

export type ProviderEvent =
  | { type: 'ProviderRegistered'; provider: string; stake: bigint }
  | { type: 'ProviderSettingsUpdated'; provider: string }
  | { type: 'StakeAdded'; provider: string; amount: bigint }
  | { type: 'AgreementCreated'; bucketId: number; provider: string }
  | { type: 'BucketCheckpointed'; bucketId: number; mmrRoot: string }
  | { type: 'ChallengeCreated'; challengeId: number; provider: string }
  | { type: 'ChallengeResponded'; challengeId: number }
  | { type: 'ProviderSlashed'; provider: string; amount: bigint }

/**
 * Subscribe to provider-related events
 */
export function subscribeToProviderEvents(
  address: string,
  callback: (event: ProviderEvent) => void
): () => void {
  if (!client) {
    console.warn('Cannot subscribe to events: not connected')
    return () => {}
  }

  // Placeholder - actual implementation would subscribe to chain events
  // and filter for events related to the given address

  return () => {}
}
