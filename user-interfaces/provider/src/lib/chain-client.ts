/**
 * Chain Client - Direct blockchain interaction via WebSocket
 *
 * This module provides direct chain access for:
 * - Provider registration and settings
 * - Querying on-chain provider state
 * - Submitting extrinsics with wallet signer
 *
 * Uses polkadot-api for queries and @polkadot/api for transactions
 * (polkadot-api's unsafeApi has issues with complex struct types)
 */

import { createClient, PolkadotClient } from 'polkadot-api'
import { getWsProvider } from 'polkadot-api/ws'
import { InjectedPolkadotAccount } from 'polkadot-api/pjs-signer'
import { BehaviorSubject } from 'rxjs'
import { ApiPromise, WsProvider } from '@polkadot/api'
import { Keyring } from '@polkadot/keyring'
import type { KeyringPair } from '@polkadot/keyring/types'
import type { Signer } from '@polkadot/api/types'

// Re-export types
export type { PolkadotClient }

// Chain client singleton (polkadot-api for queries)
let client: PolkadotClient | null = null
let wsProvider: ReturnType<typeof getWsProvider> | null = null
let unsafeApi: any = null

// @polkadot/api for transactions (more reliable with complex types)
let polkadotApi: ApiPromise | null = null
let currentEndpoint: string = ''

// Connection state
export const clientReady$ = new BehaviorSubject<boolean>(false)
export const blockNumber$ = new BehaviorSubject<number | undefined>(undefined)

// ─────────────────────────────────────────────────────────────────────────────
// Connection Management
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Connect to the blockchain via WebSocket
 */
export async function connectToChain(endpoint: string): Promise<PolkadotClient> {
  if (client && currentEndpoint === endpoint) {
    return client
  }

  try {
    currentEndpoint = endpoint

    // Connect polkadot-api for queries
    wsProvider = getWsProvider(endpoint)
    client = createClient(wsProvider)
    unsafeApi = client.getUnsafeApi()

    // Connect @polkadot/api for transactions (with timeout to avoid hanging)
    const pjsWsProvider = new WsProvider(endpoint, /* autoConnectMs */ 1000)
    const CONNECTION_TIMEOUT_MS = 10_000
    let timeoutId: ReturnType<typeof setTimeout> | undefined
    polkadotApi = await Promise.race([
      ApiPromise.create({ provider: pjsWsProvider }),
      new Promise<never>((_, reject) => {
        timeoutId = setTimeout(() => {
          pjsWsProvider.disconnect()
          reject(new Error(`Connection to ${endpoint} timed out after ${CONNECTION_TIMEOUT_MS / 1000}s`))
        }, CONNECTION_TIMEOUT_MS)
      }),
    ])
    clearTimeout(timeoutId)
    console.log('@polkadot/api connected to', endpoint)

    clientReady$.next(true)
    return client
  } catch (error) {
    clientReady$.next(false)
    throw error
  }
}

/**
 * Get @polkadot/api instance for transactions
 */
export function getPolkadotApi(): ApiPromise | null {
  return polkadotApi
}

/**
 * Disconnect from the blockchain
 */
export function disconnectFromChain(): void {
  if (client) {
    client.destroy()
    client = null
  }
  if (polkadotApi) {
    polkadotApi.disconnect()
    polkadotApi = null
  }
  wsProvider = null
  unsafeApi = null
  currentEndpoint = ''
  clientReady$.next(false)
  blockNumber$.next(undefined)
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

/**
 * Fetch chain properties (token decimals, symbol, block time, min provider stake).
 * Returns runtime-derived values instead of hardcoded constants.
 */
export async function getChainProperties(): Promise<{
  tokenDecimals: number
  tokenSymbol: string
  blockTimeMs: number
  minProviderStake: bigint
}> {
  // Defaults matching the current runtime
  let tokenDecimals = 12
  let tokenSymbol = 'UNIT'
  let blockTimeMs = 6000
  let minProviderStake = 1_000_000_000_000_000n // 1000 tokens at 12 decimals

  if (polkadotApi) {
    try {
      const props = await polkadotApi.rpc.system.properties()
      const decimals = props.tokenDecimals.unwrapOr(undefined)
      if (decimals && decimals.length > 0) tokenDecimals = decimals[0]!.toNumber()
      const symbols = props.tokenSymbol.unwrapOr(undefined)
      if (symbols && symbols.length > 0) tokenSymbol = symbols[0]!.toString()
    } catch { /* use defaults */ }

    try {
      const minStake = (polkadotApi.consts.storageProvider as any).minProviderStake
      if (minStake) minProviderStake = BigInt(minStake.toString())
    } catch { /* use default */ }

    try {
      const expectedBlockTime = (polkadotApi.consts as any).timestamp?.minimumPeriod
      if (expectedBlockTime) blockTimeMs = Number(expectedBlockTime.toString()) * 2
    } catch { /* use default */ }
  }

  return { tokenDecimals, tokenSymbol, blockTimeMs, minProviderStake }
}

// ─────────────────────────────────────────────────────────────────────────────
// Signer Helpers for @polkadot/api
// ─────────────────────────────────────────────────────────────────────────────

// Well-known dev accounts: name -> SS58 address
export const DEV_ACCOUNTS: Record<string, string> = {
  Alice: '5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY',
  Bob: '5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty',
  Charlie: '5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y',
  Dave: '5DAAnrj7VHTznn2AWBemMuyBwZWs6FNFjdyVXUeYum3PTXFy',
  Eve: '5HGjWAeFDfFCWPsjFQdVV2Msvz2XtMktvgocEZcCj68kUMaw',
  Ferdie: '5CiPPseXPECbkjWCa6MnjNokrgYjMqmKndv2rSnekmSK2DjL',
}

// Reverse map: address -> URI (derived from DEV_ACCOUNTS)
const DEV_ACCOUNT_URIS: Record<string, string> = Object.fromEntries(
  Object.entries(DEV_ACCOUNTS).map(([name, addr]) => [addr, `//${name}`])
)

/**
 * Get a KeyringPair for dev accounts, or create a custom signer for extension accounts
 */
function getPolkadotApiSigner(
  account: InjectedPolkadotAccount
): { keypair: KeyringPair } | { address: string; signer: Signer } {
  const address = account.address

  // Check if this is a dev account
  const devUri = DEV_ACCOUNT_URIS[address]
  if (devUri) {
    const keyring = new Keyring({ type: 'sr25519' })
    const keypair = keyring.addFromUri(devUri)
    return { keypair }
  }

  // For extension accounts, create a custom signer that wraps polkadot-api's signer
  // Note: This is a simplified implementation for extension signing
  // For production, you'd want to use the extension's native signRaw/signPayload
  const customSigner: Signer = {
    signPayload: async (payload) => {
      // polkadot-api's signer uses signTx for extrinsics
      const signerPayload = account.polkadotSigner

      // The method field contains the encoded call data
      const methodHex = payload.method || ''
      const toSign = hexToBytes(methodHex)

      const signature = await signerPayload.signBytes(toSign)
      return {
        id: 0,
        signature: bytesToHex(signature),
      }
    },
  }

  return { address, signer: customSigner }
}

function hexToBytes(hex: string): Uint8Array {
  const cleanHex = hex.startsWith('0x') ? hex.slice(2) : hex
  const bytes = new Uint8Array(cleanHex.length / 2)
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(cleanHex.substr(i * 2, 2), 16)
  }
  return bytes
}

function bytesToHex(bytes: Uint8Array): `0x${string}` {
  return `0x${Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('')}`
}

// Transaction status for progress callbacks
export type TxStatus =
  | { type: 'signing'; message: string }
  | { type: 'broadcast'; message: string }
  | { type: 'inBlock'; message: string; blockHash: string }
  | { type: 'finalized'; message: string; blockHash: string }
  | { type: 'error'; message: string }

export type TxProgressCallback = (status: TxStatus) => void

/**
 * Helper to sign and submit a transaction and wait for inclusion in block
 * Uses isInBlock instead of isFinalized for faster response
 */
async function signAndSubmitTx(
  tx: any,
  signer: InjectedPolkadotAccount,
  description: string,
  onProgress?: TxProgressCallback
): Promise<void> {
  const signerInfo = getPolkadotApiSigner(signer)

  return new Promise<void>((resolve, reject) => {
    let unsub: (() => void) | null = null
    let resolved = false

    const cleanup = () => {
      if (unsub) {
        try {
          unsub()
        } catch (e) {
          console.warn('Failed to unsubscribe:', e)
        }
        unsub = null
      }
    }

    const callback = (result: any) => {
      const statusType = result.status.type
      console.log(`${description} status:`, statusType, result.status.toString())

      // Report progress for different status types
      if (result.status.isReady) {
        onProgress?.({ type: 'signing', message: `Signing ${description.toLowerCase()}...` })
      } else if (result.status.isBroadcast) {
        onProgress?.({ type: 'broadcast', message: 'Transaction broadcast to network...' })
      }

      // Check for errors first
      if (result.dispatchError) {
        let errorMsg = 'Transaction failed'
        if (result.dispatchError.isModule) {
          try {
            const decoded = polkadotApi!.registry.findMetaError(result.dispatchError.asModule)
            errorMsg = `${decoded.section}.${decoded.name}: ${decoded.docs.join(' ')}`
          } catch (e) {
            errorMsg = result.dispatchError.toString()
          }
        } else {
          errorMsg = result.dispatchError.toString()
        }
        onProgress?.({ type: 'error', message: errorMsg })
        cleanup()
        if (!resolved) {
          resolved = true
          reject(new Error(`Transaction failed: ${errorMsg}`))
        }
        return
      }

      // Success when included in block (faster than waiting for finalization)
      if (result.status.isInBlock) {
        const blockHash = result.status.asInBlock.toHex()
        console.log(`${description} included in block:`, blockHash)
        onProgress?.({
          type: 'inBlock',
          message: `${description} included in block`,
          blockHash,
        })
        cleanup()
        if (!resolved) {
          resolved = true
          resolve()
        }
        return
      }

      // Also handle finalized as success
      if (result.status.isFinalized) {
        const blockHash = result.status.asFinalized.toHex()
        console.log(`${description} finalized in block:`, blockHash)
        onProgress?.({
          type: 'finalized',
          message: `${description} finalized`,
          blockHash,
        })
        cleanup()
        if (!resolved) {
          resolved = true
          resolve()
        }
        return
      }
    }

    const startSubmission = async () => {
      try {
        if ('keypair' in signerInfo) {
          unsub = await tx.signAndSend(signerInfo.keypair, { nonce: -1 }, callback)
        } else {
          unsub = await tx.signAndSend(
            signerInfo.address,
            { signer: signerInfo.signer, nonce: -1 },
            callback
          )
        }
      } catch (err) {
        if (!resolved) {
          resolved = true
          reject(err)
        }
      }
    }

    startSubmission()
  })
}

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

export interface OnChainProviderInfo {
  stake: bigint
  capacity?: bigint
  usedCapacity?: bigint
  activeBuckets: number
  registeredAt: number
  multiaddr?: string
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

export interface OnChainAgreement {
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
  paymentLocked: bigint
}

export interface OnChainAgreementRequest {
  bucketId: number
  requester: string
  maxBytes: bigint
  paymentLocked: bigint
  duration: number
  expiresAt: number
}

export interface OnChainCheckpoint {
  bucketId: number
  mmrRoot: string
  leafCount: number
  submittedAt: number
  blockNumber: number
  providers: string[]
}

export interface OnChainChallenge {
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

export interface OnChainBucketMember {
  account: string
  role: 'Admin' | 'Writer' | 'Reader'
}

export interface OnChainBucketSnapshot {
  mmrRoot: string
  startSeq: number
  leafCount: number
  checkpointBlock: number
}

export interface OnChainCheckpointConfig {
  interval: number
  gracePeriod: number
  enabled: boolean
}

export interface OnChainBucketDetails {
  bucketId: number
  members: OnChainBucketMember[]
  frozenStartSeq: number | null
  minProviders: number
  primaryProviders: string[]
  snapshot: OnChainBucketSnapshot | null
  historicalRoots: Array<{ position: number; root: string }>
  totalSnapshots: number
  checkpointConfig: OnChainCheckpointConfig | null
  lastCheckpointWindow: number | null
  checkpointPoolBalance: bigint
  checkpointReward: bigint
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider Pallet Queries
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Check if an account is registered as a provider
 */
export async function isProviderRegistered(address: string): Promise<boolean> {
  if (!client || !unsafeApi) throw new Error('Not connected to chain')

  try {
    const provider = await unsafeApi.query.StorageProvider.Providers.getValue(address)
    console.log('Provider query result for', address, ':', provider)
    return provider !== undefined && provider !== null
  } catch (error) {
    console.error('Error checking provider registration:', error)
    return false
  }
}

/**
 * Get provider info and settings from chain in a single query.
 * Both are stored in the same Providers storage entry.
 */
export async function getProviderData(
  address: string
): Promise<{ info: OnChainProviderInfo; settings: OnChainProviderSettings } | null> {
  if (!client || !unsafeApi) throw new Error('Not connected to chain')

  try {
    const provider = await unsafeApi.query.StorageProvider.Providers.getValue(address)
    if (!provider) return null

    console.log('Provider data:', provider)

    // Decode multiaddr bytes to string
    let multiaddr: string | undefined
    if (provider.multiaddr) {
      try {
        const bytes = provider.multiaddr instanceof Uint8Array
          ? provider.multiaddr
          : new Uint8Array(provider.multiaddr)
        multiaddr = new TextDecoder().decode(bytes)
      } catch {
        // Ignore decode errors
      }
    }

    const info: OnChainProviderInfo = {
      stake: BigInt(provider.stake?.toString() || '0'),
      capacity: provider.settings?.max_capacity ? BigInt(provider.settings.max_capacity.toString()) : undefined,
      usedCapacity: provider.committed_bytes ? BigInt(provider.committed_bytes.toString()) : undefined,
      activeBuckets: provider.stats?.agreements_total || 0,
      registeredAt: provider.stats?.registered_at || 0,
      multiaddr,
    }

    const s = provider.settings
    const settings: OnChainProviderSettings | null = s
      ? {
          minDuration: Number(s.min_duration ?? s.minDuration ?? 0),
          maxDuration: Number(s.max_duration ?? s.maxDuration ?? 0),
          pricePerByte: BigInt((s.price_per_byte ?? s.pricePerByte ?? 0).toString()),
          acceptingPrimary: s.accepting_primary ?? s.acceptingPrimary ?? false,
          acceptingReplica: s.accepting_replica ?? s.acceptingReplica ?? false,
          replicaSyncPrice: (s.replica_sync_price ?? s.replicaSyncPrice)
            ? BigInt((s.replica_sync_price ?? s.replicaSyncPrice).toString())
            : null,
          acceptingExtensions: s.accepting_extensions ?? s.acceptingExtensions ?? false,
          maxCapacity: BigInt((s.max_capacity ?? s.maxCapacity ?? 0).toString()),
        }
      : null
    console.log('[settings] Parsed settings:', JSON.stringify(settings, (_, v) => typeof v === 'bigint' ? v.toString() : v))

    if (!settings) return null

    return { info, settings }
  } catch (error) {
    console.error('Error fetching provider data:', error)
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
    return null
  }
}

/**
 * Get all agreements for a provider
 */
export async function getProviderAgreements(address: string): Promise<OnChainAgreement[]> {
  if (!client || !unsafeApi) throw new Error('Not connected to chain')

  try {
    const agreements: OnChainAgreement[] = []

    try {
      // StorageAgreements is DoubleMap<BucketId, AccountId, StorageAgreement>
      const entries = await unsafeApi.query.StorageProvider.StorageAgreements.getEntries()
      const currentBlock = blockNumber$.getValue() || 0

      console.log('[agreements] Got', entries.length, 'StorageAgreements entries, filtering for', address)

      for (const entry of entries) {
        const { keyArgs: key, value } = entry as { keyArgs: any[]; value: any }
        console.log('[agreements] Entry key:', JSON.stringify(key, (_, v) => typeof v === 'bigint' ? v.toString() : v), 'value:', JSON.stringify(value, (_, v) => typeof v === 'bigint' ? v.toString() : v))
        if (!value) continue

        // key: [bucketId, provider]
        const bucketId = Number(key[0]) || 0
        const provider = key[1]?.toString() || ''

        // Filter by our provider
        if (provider !== address) continue

        const expiresAt = value.expires_at || 0
        let status: 'active' | 'expired' | 'terminated' = 'active'
        if (value.extensions_blocked) {
          status = 'terminated'
        } else if (currentBlock > expiresAt && expiresAt > 0) {
          status = 'expired'
        }

        // Determine if primary from role enum
        const role = value.role
        const isPrimary = role?.type === 'Primary' || role?.tag === 'Primary' ||
          (typeof role === 'string' && role === 'Primary') ||
          (!role?.type && !role?.tag) // default to primary if unknown

        agreements.push({
          id: bucketId,
          bucketId,
          provider,
          user: value.owner?.toString() || '',
          maxBytes: BigInt(value.max_bytes?.toString() || '0'),
          pricePerByte: BigInt(value.price_per_byte?.toString() || '0'),
          paymentLocked: BigInt(value.payment_locked?.toString() || '0'),
          startBlock: value.started_at || 0,
          endBlock: expiresAt,
          isPrimary,
          status,
        })
      }
    } catch (e) {
      console.warn('Could not query StorageAgreements:', e)
    }

    return agreements
  } catch (error) {
    console.error('Error fetching provider agreements:', error)
    return []
  }
}

/**
 * Get pending agreement requests for a provider
 */
export async function getAgreementRequests(address: string): Promise<OnChainAgreementRequest[]> {
  if (!client || !unsafeApi) throw new Error('Not connected to chain')

  try {
    const requests: OnChainAgreementRequest[] = []

    try {
      // AgreementRequests is a DoubleMap: provider -> bucket_id -> request
      // Fetch all entries and filter by provider, same pattern as getProviderAgreements
      const entries = await unsafeApi.query.StorageProvider.AgreementRequests.getEntries()

      console.log('[agreementRequests] Got', entries.length, 'AgreementRequests entries, filtering for', address)

      for (const entry of entries) {
        const { keyArgs: key, value } = entry as { keyArgs: any[]; value: any }
        console.log('[agreementRequests] Entry key:', JSON.stringify(key, (_, v) => typeof v === 'bigint' ? v.toString() : v), 'value:', JSON.stringify(value, (_, v) => typeof v === 'bigint' ? v.toString() : v))
        if (!value) continue

        // key structure for full getEntries(): [provider, bucketId]
        const provider = key[0]?.toString()
        if (provider !== address) continue

        const bucketId = Number(key[1]) || 0

        requests.push({
          bucketId,
          requester: value.requester?.toString() || '',
          maxBytes: BigInt(value.max_bytes?.toString() || '0'),
          paymentLocked: BigInt(value.payment_locked?.toString() || '0'),
          duration: value.duration || 0,
          expiresAt: value.expires_at || 0,
        })
      }
    } catch (e) {
      console.warn('Could not query agreement requests:', e)
    }

    return requests
  } catch (error) {
    console.error('Error fetching agreement requests:', error)
    return []
  }
}

/**
 * Get checkpoints for buckets where provider has agreements
 */
export async function getProviderCheckpoints(address: string): Promise<OnChainCheckpoint[]> {
  if (!client || !unsafeApi) throw new Error('Not connected to chain')

  try {
    const checkpoints: OnChainCheckpoint[] = []

    // First get agreements to find bucket IDs
    const agreements = await getProviderAgreements(address)
    const bucketIds = [...new Set(agreements.map((a) => a.bucketId))]

    // Query checkpoints from bucket snapshot field (use @polkadot/api to avoid PAPI incompatibility)
    for (const bucketId of bucketIds) {
      try {
        if (!polkadotApi) continue
        const bucketOpt = await polkadotApi.query.storageProvider.buckets(bucketId)
        if ((bucketOpt as any).isNone) continue
        const bucket = (bucketOpt as any).unwrap()
        const snap = (bucket as any).snapshot
        if (snap && !snap.isNone) {
          const s = snap.isNone === undefined ? snap : snap.unwrap()
          checkpoints.push({
            bucketId,
            mmrRoot: s.mmrRoot?.toHex?.() || s.mmr_root?.toString() || '0x',
            leafCount: Number(s.leafCount?.toNumber?.() ?? s.leaf_count ?? 0),
            submittedAt: Number(s.checkpointBlock?.toNumber?.() ?? s.checkpoint_block ?? 0),
            blockNumber: Number(s.checkpointBlock?.toNumber?.() ?? s.checkpoint_block ?? 0),
            providers: (bucket as any).primaryProviders?.map((p: any) => p.toString()) || [],
          })
        }
      } catch (e) {
        console.warn(`Could not query checkpoint for bucket ${bucketId}:`, e)
      }
    }

    return checkpoints
  } catch (error) {
    console.error('Error fetching checkpoints:', error)
    return []
  }
}

/**
 * Get detailed bucket information for the given bucket IDs.
 * Combines Bucket struct data with checkpoint config, pool, and rewards.
 */
export async function getBucketDetails(
  bucketIds: number[],
  providerAddress: string
): Promise<OnChainBucketDetails[]> {
  if (!polkadotApi || !unsafeApi) throw new Error('Not connected to chain')

  const details: OnChainBucketDetails[] = []

  for (const bucketId of bucketIds) {
    try {
      // Query Bucket struct via @polkadot/api (handles Option/BoundedVec correctly)
      const bucketOpt = await polkadotApi.query.storageProvider.buckets(bucketId)
      if ((bucketOpt as any).isNone) continue
      const bucket = (bucketOpt as any).unwrap()

      // Parse members
      const members: OnChainBucketMember[] = []
      const rawMembers = (bucket as any).members || []
      for (const m of rawMembers) {
        const roleStr = m.role?.toString?.() || m.role?.type || 'Reader'
        members.push({
          account: m.account?.toString() || '',
          role: roleStr as 'Admin' | 'Writer' | 'Reader',
        })
      }

      // Parse snapshot
      let snapshot: OnChainBucketSnapshot | null = null
      const snap = (bucket as any).snapshot
      if (snap && !snap.isNone) {
        const s = snap.isNone === undefined ? snap : snap.unwrap()
        snapshot = {
          mmrRoot: s.mmrRoot?.toHex?.() || s.mmr_root?.toString?.() || '0x',
          startSeq: Number(s.startSeq?.toNumber?.() ?? s.start_seq ?? 0),
          leafCount: Number(s.leafCount?.toNumber?.() ?? s.leaf_count ?? 0),
          checkpointBlock: Number(s.checkpointBlock?.toNumber?.() ?? s.checkpoint_block ?? 0),
        }
      }

      // Parse primary providers
      const primaryProviders = ((bucket as any).primaryProviders || (bucket as any).primary_providers || [])
        .map((p: any) => p.toString())

      // Parse historical roots (skip zero entries)
      const historicalRoots: Array<{ position: number; root: string }> = []
      const rawRoots = (bucket as any).historicalRoots || (bucket as any).historical_roots || []
      for (const entry of rawRoots) {
        const pos = Number(entry[0]?.toNumber?.() ?? entry[0] ?? 0)
        const root = entry[1]?.toHex?.() || entry[1]?.toString?.() || '0x'
        if (pos > 0 || (root !== '0x' && !root.match(/^0x0+$/))) {
          historicalRoots.push({ position: pos, root })
        }
      }

      // Checkpoint sub-storage via PAPI unsafeApi
      let checkpointConfig: OnChainCheckpointConfig | null = null
      try {
        const config = await unsafeApi.query.StorageProvider.CheckpointConfigs.getValue(bucketId)
        if (config) {
          checkpointConfig = {
            interval: Number(config.interval ?? 0),
            gracePeriod: Number(config.grace_period ?? config.gracePeriod ?? 0),
            enabled: config.enabled ?? true,
          }
        }
      } catch { /* not configured */ }

      let lastCheckpointWindow: number | null = null
      try {
        const val = await unsafeApi.query.StorageProvider.LastCheckpointWindow.getValue(bucketId)
        lastCheckpointWindow = val != null ? Number(val) : null
      } catch { /* not set */ }

      let checkpointPoolBalance = 0n
      try {
        const pool = await unsafeApi.query.StorageProvider.CheckpointPool.getValue(bucketId)
        checkpointPoolBalance = pool != null ? BigInt(pool.toString()) : 0n
      } catch { /* no pool */ }

      let checkpointReward = 0n
      try {
        const reward = await unsafeApi.query.StorageProvider.CheckpointRewards.getValue(
          bucketId,
          providerAddress
        )
        checkpointReward = reward != null ? BigInt(reward.toString()) : 0n
      } catch { /* no reward */ }

      details.push({
        bucketId,
        members,
        frozenStartSeq: (bucket as any).frozenStartSeq?.toNumber?.() ??
          ((bucket as any).frozen_start_seq != null ? Number((bucket as any).frozen_start_seq) : null),
        minProviders: Number((bucket as any).minProviders?.toNumber?.() ?? (bucket as any).min_providers ?? 0),
        primaryProviders,
        snapshot,
        historicalRoots,
        totalSnapshots: Number((bucket as any).totalSnapshots?.toNumber?.() ?? (bucket as any).total_snapshots ?? 0),
        checkpointConfig,
        lastCheckpointWindow,
        checkpointPoolBalance,
        checkpointReward,
      })
    } catch (e) {
      console.warn(`Could not query bucket details for bucket ${bucketId}:`, e)
    }
  }

  return details
}

/**
 * Get challenges for a provider
 */
export async function getProviderChallenges(address: string): Promise<OnChainChallenge[]> {
  if (!client || !unsafeApi) throw new Error('Not connected to chain')

  const challenges = new Map<string, OnChainChallenge>() // keyed by "deadline-index"
  const currentBlock = blockNumber$.getValue() || 0

  // Check storage for any still-pending challenges.
  // Note: responded/slashed challenges are removed from storage — those are
  // captured by the real-time subscribeToChallengeEvents() subscription instead.
  try {
    const entries = await unsafeApi.query.StorageProvider.Challenges.getEntries()
    for (const entry of entries) {
      const { keyArgs, value: challengeVec } = entry as { keyArgs: any[]; value: any }
      if (!challengeVec) continue
      const deadline = Number(keyArgs[0]) || 0
      const items = Array.isArray(challengeVec) ? challengeVec : [challengeVec]
      for (let idx = 0; idx < items.length; idx++) {
        const ch = items[idx]
        if (!ch) continue
        const challengeProvider = ch.provider?.toString()
        if (challengeProvider !== address) continue
        const key = `${deadline}-${idx}`
        if (!challenges.has(key)) {
          challenges.set(key, {
            id: idx,
            bucketId: Number(ch.bucket_id ?? 0),
            challenger: ch.challenger?.toString() || '',
            provider: challengeProvider,
            leafIndex: Number(ch.leaf_index ?? 0),
            status: currentBlock > deadline ? 'expired' : 'pending',
            createdAt: 0,
            deadline,
          })
        }
      }
    }
  } catch (e) {
    console.warn('Could not query challenge storage:', e)
  }

  // Sort by deadline descending (most recent first)
  return Array.from(challenges.values()).sort((a, b) => b.deadline - a.deadline)
}

// ─────────────────────────────────────────────────────────────────────────────
// Extrinsic Submission (using @polkadot/api)
// ─────────────────────────────────────────────────────────────────────────────

export interface RegisterProviderParams {
  stake: bigint
  multiaddr: string // e.g., "/ip4/127.0.0.1/tcp/3000"
  publicKey?: Uint8Array // Optional - defaults to signer's public key
}

/**
 * Submit register_provider extrinsic using @polkadot/api
 */
export async function submitRegisterProvider(
  params: RegisterProviderParams,
  settings: OnChainProviderSettings,
  signer: InjectedPolkadotAccount,
  onProgress?: TxProgressCallback
): Promise<void> {
  if (!polkadotApi) throw new Error('Not connected to chain')

  // Use signer's public key if not provided
  const publicKey = params.publicKey || signer.polkadotSigner.publicKey

  // Convert multiaddr string to bytes
  const multiaddrBytes = new TextEncoder().encode(params.multiaddr)

  console.log('Register provider params:', {
    multiaddr: params.multiaddr,
    publicKeyLength: publicKey?.length,
    stake: params.stake.toString(),
  })

  // Ensure we have a valid public key
  if (!publicKey) {
    throw new Error('No public key available for signing')
  }

  console.log('Building transaction with @polkadot/api...')

  // Build transaction using @polkadot/api
  // register_provider(multiaddr, public_key, stake)
  const tx = polkadotApi.tx.storageProvider.registerProvider(
    Array.from(multiaddrBytes), // BoundedVec<u8>
    Array.from(publicKey), // BoundedVec<u8>
    params.stake.toString() // Balance
  )

  console.log('Transaction built successfully')
  console.log('Submitting register_provider transaction...')

  onProgress?.({ type: 'signing', message: 'Signing registration transaction...' })
  await signAndSubmitTx(tx, signer, 'Register provider', onProgress)

  console.log('Registration complete, now updating settings...')

  // After registration, submit settings
  onProgress?.({ type: 'signing', message: 'Now updating provider settings...' })
  await submitUpdateSettings(settings, signer, onProgress)
}

/**
 * Submit update_provider_settings extrinsic using @polkadot/api
 */
export async function submitUpdateSettings(
  settings: OnChainProviderSettings,
  signer: InjectedPolkadotAccount,
  onProgress?: TxProgressCallback
): Promise<void> {
  if (!polkadotApi) throw new Error('Not connected to chain')

  // Build settings object matching the pallet's ProviderSettings struct
  // For @polkadot/api, we pass the struct fields as an object
  const palletSettings = {
    min_duration: settings.minDuration,
    max_duration: settings.maxDuration,
    price_per_byte: settings.pricePerByte.toString(),
    accepting_primary: settings.acceptingPrimary,
    // replica_sync_price: Some(value) if accepting replicas, None otherwise
    replica_sync_price:
      settings.acceptingReplica && settings.replicaSyncPrice
        ? settings.replicaSyncPrice.toString()
        : null,
    accepting_extensions: settings.acceptingExtensions,
    max_capacity: settings.maxCapacity.toString(),
  }

  console.log('Submitting update_provider_settings with @polkadot/api:', palletSettings)

  const tx = polkadotApi.tx.storageProvider.updateProviderSettings(palletSettings)

  await signAndSubmitTx(tx, signer, 'Update settings', onProgress)

  console.log('Settings updated successfully')
}

/**
 * Submit update_provider_multiaddr extrinsic using @polkadot/api
 */
export async function submitUpdateMultiaddr(
  multiaddr: string,
  signer: InjectedPolkadotAccount,
  onProgress?: TxProgressCallback
): Promise<void> {
  if (!polkadotApi) throw new Error('Not connected to chain')

  const multiaddrBytes = new TextEncoder().encode(multiaddr)

  console.log('Submitting update_provider_multiaddr with @polkadot/api:', multiaddr)

  const tx = polkadotApi.tx.storageProvider.updateProviderMultiaddr(
    Array.from(multiaddrBytes)
  )

  await signAndSubmitTx(tx, signer, 'Update multiaddr', onProgress)

  console.log('Multiaddr updated successfully')
}

/**
 * Submit add_stake extrinsic using @polkadot/api
 */
export async function submitAddStake(
  amount: bigint,
  signer: InjectedPolkadotAccount
): Promise<void> {
  if (!polkadotApi) throw new Error('Not connected to chain')

  console.log('Submitting add_stake transaction with amount:', amount.toString())

  const tx = polkadotApi.tx.storageProvider.addStake(amount.toString())

  await signAndSubmitTx(tx, signer, 'Add stake')

  console.log('Stake added successfully')
}

/**
 * Submit respond_to_challenge extrinsic using @polkadot/api
 */
export async function submitChallengeResponse(
  challengeId: number,
  proof: Uint8Array,
  signer: InjectedPolkadotAccount
): Promise<void> {
  if (!polkadotApi) throw new Error('Not connected to chain')

  console.log('Submitting respond_to_challenge transaction for challenge:', challengeId)

  const tx = polkadotApi.tx.storageProvider.respondToChallenge(challengeId, Array.from(proof))

  await signAndSubmitTx(tx, signer, 'Challenge response')

  console.log('Challenge response submitted successfully')
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
    return () => {}
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
 * Subscribe to challenge events in real-time via finalizedBlock$.
 * Processes each finalized block's events while the block is still pinned.
 * Returns an unsubscribe function.
 */
export function subscribeToChallengeEvents(
  address: string,
  onChallenge: (challenge: OnChainChallenge) => void
): () => void {
  if (!client || !unsafeApi) {
    console.warn('Cannot subscribe to challenge events: not connected')
    return () => {}
  }

  const api = unsafeApi
  const subscription = client.finalizedBlock$.subscribe({
    next: async (block) => {
      try {
        const rawEvents = await api.query.System.Events.getValue({ at: block.hash })
        const events = Array.isArray(rawEvents) ? rawEvents : (rawEvents as any)?.value ?? []

        // Decode extrinsics to detect challenge type from call name.
        // Try decoding call data from the end of each extrinsic (works for
        // both signed and unsigned since call data is the tail).
        const extrinsicTypes: Map<number, string> = new Map()
        try {
          const body = await client!.getBlockBody(block.hash)
          for (let i = 0; i < body.length; i++) {
            try {
              const raw = body[i]
              const bytes = raw instanceof Uint8Array ? raw : new TextEncoder().encode(String(raw))
              // Skip compact length prefix
              const mode = bytes[0]! & 0x03
              const lenSize = mode === 0 ? 1 : mode === 1 ? 2 : mode === 2 ? 4 : 1 + ((bytes[0]! >> 2) + 4)
              // Try slices starting from the longest (right after signature) to
              // shortest. Longest-first avoids false positives like System.remark
              // which matches almost any short byte sequence.
              const minOffset = lenSize + 1 + 1 + 32 + 1 + 64 // preamble+addrType+addr+sigType+sig
              for (let off = minOffset; off < bytes.length - 2; off++) {
                try {
                  const tx = await api.txFromCallData(bytes.slice(off))
                  const pallet = tx.decodedCall?.type || ''
                  const callName = (tx.decodedCall?.value as any)?.type || ''
                  if (pallet === 'StorageProvider') {
                    console.log(`[challenges] Decoded ext #${i}: ${pallet}.${callName}`)
                    if (callName === 'challenge_offchain') extrinsicTypes.set(i, 'offchain')
                    else if (callName === 'challenge_checkpoint') extrinsicTypes.set(i, 'checkpoint')
                  }
                  break // first successful decode is the correct one
                } catch { /* try next offset */ }
              }
            } catch { /* skip */ }
          }
        } catch { /* body not available */ }

        for (const record of events) {
          const event = (record as any).event
          if (!event || event.type !== 'StorageProvider') continue
          const ev = event.value
          if (!ev) continue

          // Get extrinsic index from event phase for type detection
          const phase = (record as any).phase
          const extIdx = phase?.type === 'ApplyExtrinsic' ? Number(phase.value) : -1

          if (ev.type === 'ChallengeCreated') {
            const d = ev.value
            const provider = d.provider?.toString()
            if (provider !== address) continue
            const cid = d.challenge_id
            // Determine challenge type from the extrinsic that created it
            const challengeType = extrinsicTypes.get(extIdx) as 'offchain' | 'checkpoint' | undefined
            onChallenge({
              id: Number(cid.index ?? 0),
              bucketId: Number(d.bucket_id ?? 0),
              challenger: d.challenger?.toString() || '',
              provider,
              leafIndex: Number(d.leaf_index ?? 0),
              status: 'pending',
              challengeType: challengeType || 'unknown',
              createdAt: block.number,
              deadline: Number(cid.deadline ?? d.respond_by ?? 0),
            })
          } else if (ev.type === 'ChallengeDefended') {
            const d = ev.value
            if (d.provider?.toString() !== address) continue
            const cid = d.challenge_id
            onChallenge({
              id: Number(cid.index ?? 0),
              bucketId: 0,
              challenger: '',
              provider: address,
              leafIndex: 0,
              status: 'responded',
              createdAt: 0,
              deadline: Number(cid.deadline ?? 0),
            })
          } else if (ev.type === 'ChallengeSlashed') {
            const d = ev.value
            if (d.provider?.toString() !== address) continue
            const cid = d.challenge_id
            onChallenge({
              id: Number(cid.index ?? 0),
              bucketId: 0,
              challenger: '',
              provider: address,
              leafIndex: 0,
              status: 'slashed',
              createdAt: 0,
              deadline: Number(cid.deadline ?? 0),
            })
          }
        }
      } catch {
        // Block unpinned or events unavailable — skip
      }
    },
    error: (err) => {
      console.error('Challenge event subscription error:', err)
    },
  })

  return () => subscription.unsubscribe()
}

/**
 * Subscribe to provider-related events (stub for general events)
 */
export function subscribeToProviderEvents(
  _address: string,
  _callback: (event: ProviderEvent) => void
): () => void {
  if (!client) return () => {}
  // General event subscription — challenge events are handled by subscribeToChallengeEvents
  return () => {}
}
