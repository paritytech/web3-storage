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
import { getWsProvider } from 'polkadot-api/ws-provider/web'
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
  createdAt: number
  deadline: number
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

    const info: OnChainProviderInfo = {
      stake: BigInt(provider.stake?.toString() || '0'),
      capacity: provider.capacity ? BigInt(provider.capacity.toString()) : undefined,
      usedCapacity: provider.used_capacity ? BigInt(provider.used_capacity.toString()) : undefined,
      activeBuckets: provider.active_buckets || 0,
      registeredAt: provider.registered_at || 0,
    }

    const settings: OnChainProviderSettings | null = provider.settings
      ? {
          minDuration: provider.settings.min_duration || 0,
          maxDuration: provider.settings.max_duration || 0,
          pricePerByte: BigInt(provider.settings.price_per_byte?.toString() || '0'),
          acceptingPrimary: provider.settings.accepting_primary ?? false,
          acceptingReplica: provider.settings.accepting_replica ?? false,
          replicaSyncPrice: provider.settings.replica_sync_price
            ? BigInt(provider.settings.replica_sync_price.toString())
            : null,
          acceptingExtensions: provider.settings.accepting_extensions ?? false,
          maxCapacity: BigInt(provider.settings.max_capacity?.toString() || '0'),
        }
      : null

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
    // Query agreements from storage
    // The storage structure depends on the pallet design
    const agreements: OnChainAgreement[] = []

    // Try to get agreements from Agreements storage map
    try {
      const entries = await unsafeApi.query.StorageProvider.Agreements.getEntries()
      const currentBlock = blockNumber$.getValue() || 0

      for (const [key, value] of entries) {
        if (!value) continue

        // Check if this agreement involves our provider
        const agreementProvider = value.provider?.toString()
        if (agreementProvider !== address) continue

        const endBlock = value.end_block || 0
        let status: 'active' | 'expired' | 'terminated' = 'active'
        if (value.terminated) {
          status = 'terminated'
        } else if (currentBlock > endBlock) {
          status = 'expired'
        }

        agreements.push({
          id: key[0] || 0,
          bucketId: value.bucket_id || 0,
          provider: agreementProvider,
          user: value.user?.toString() || '',
          maxBytes: BigInt(value.max_bytes?.toString() || '0'),
          pricePerByte: BigInt(value.price_per_byte?.toString() || '0'),
          startBlock: value.start_block || 0,
          endBlock,
          isPrimary: value.is_primary ?? true,
          status,
        })
      }
    } catch (e) {
      console.warn('Could not query agreements:', e)
    }

    return agreements
  } catch (error) {
    console.error('Error fetching provider agreements:', error)
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

    // Query checkpoints for each bucket
    for (const bucketId of bucketIds) {
      try {
        const snapshot = await unsafeApi.query.StorageProvider.BucketSnapshots.getValue(bucketId)
        if (snapshot) {
          checkpoints.push({
            bucketId,
            mmrRoot: snapshot.mmr_root?.toString() || '0x',
            leafCount: snapshot.leaf_count || 0,
            submittedAt: snapshot.submitted_at || 0,
            blockNumber: snapshot.block_number || 0,
            providers: snapshot.providers?.map((p: any) => p.toString()) || [],
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
 * Get challenges for a provider
 */
export async function getProviderChallenges(address: string): Promise<OnChainChallenge[]> {
  if (!client || !unsafeApi) throw new Error('Not connected to chain')

  try {
    const challenges: OnChainChallenge[] = []

    try {
      const entries = await unsafeApi.query.StorageProvider.Challenges.getEntries()
      const currentBlock = blockNumber$.getValue() || 0

      for (const [key, value] of entries) {
        if (!value) continue

        const challengeProvider = value.provider?.toString()
        if (challengeProvider !== address) continue

        const deadline = value.deadline || 0
        let status: 'pending' | 'responded' | 'slashed' | 'expired' = 'pending'
        if (value.responded) {
          status = 'responded'
        } else if (value.slashed) {
          status = 'slashed'
        } else if (currentBlock > deadline) {
          status = 'expired'
        }

        challenges.push({
          id: key[0] || 0,
          bucketId: value.bucket_id || 0,
          challenger: value.challenger?.toString() || '',
          provider: challengeProvider,
          leafIndex: value.leaf_index || 0,
          status,
          createdAt: value.created_at || 0,
          deadline,
        })
      }
    } catch (e) {
      console.warn('Could not query challenges:', e)
    }

    return challenges
  } catch (error) {
    console.error('Error fetching challenges:', error)
    return []
  }
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
 * Subscribe to provider-related events
 *
 * Note: Full event parsing requires runtime metadata.
 * This is a placeholder that logs finalized blocks.
 */
export function subscribeToProviderEvents(
  _address: string,
  _callback: (event: ProviderEvent) => void
): () => void {
  if (!client) {
    console.warn('Cannot subscribe to events: not connected')
    return () => {}
  }

  // Subscribe to finalized blocks
  // Note: Event parsing requires knowing the runtime metadata structure
  // For now, we log the block and would need to query events separately
  const subscription = client.finalizedBlock$.subscribe({
    next: (block) => {
      console.log('New finalized block:', block.number)
      // In a full implementation, we would query events from the block
      // and parse them using the chain metadata
    },
    error: (err) => {
      console.error('Event subscription error:', err)
    },
  })

  return () => subscription.unsubscribe()
}
