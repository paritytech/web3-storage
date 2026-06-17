// SPDX-License-Identifier: GPL-3.0-only

/**
 * Chain Client - Direct blockchain interaction via WebSocket.
 *
 * Single-library setup: polkadot-api (PAPI) v2 typed API for both queries and
 * tx submission. The typed API is built from descriptors regenerated against
 * the live runtime via `pnpm papi:generate`. PR-5 of user-interfaces/AUDIT.md
 * tracked the migration from the previous dual @polkadot/api + PAPI setup.
 */

import { createClient, Enum, type PolkadotClient, type Transaction, type TxFinalizedPayload, type TypedApi } from 'polkadot-api'
import { getWsProvider } from 'polkadot-api/ws'
import { type InjectedPolkadotAccount } from 'polkadot-api/pjs-signer'
import { parachain } from '@polkadot-api/descriptors'
import { BehaviorSubject } from 'rxjs'
import { getSs58Prefix, isSameAddress } from '@web3-storage/papi'

export type { PolkadotClient }
type ParachainApi = TypedApi<typeof parachain>

// ─────────────────────────────────────────────────────────────────────────────
// Connection state
// ─────────────────────────────────────────────────────────────────────────────

let client: PolkadotClient | null = null
let api: ParachainApi | null = null
let currentEndpoint: string = ''

export const clientReady$ = new BehaviorSubject<boolean>(false)
export const blockNumber$ = new BehaviorSubject<number | undefined>(undefined)

export async function connectToChain(endpoint: string): Promise<PolkadotClient> {
  if (client && currentEndpoint === endpoint) return client

  try {
    currentEndpoint = endpoint
    client = createClient(getWsProvider(endpoint))
    api = client.getTypedApi(parachain)
    clientReady$.next(true)
    return client
  } catch (error) {
    clientReady$.next(false)
    throw error
  }
}

export function disconnectFromChain(): void {
  client?.destroy()
  client = null
  api = null
  currentEndpoint = ''
  clientReady$.next(false)
  blockNumber$.next(undefined)
}

export function getClient(): PolkadotClient | null {
  return client
}

function requireApi(): ParachainApi {
  if (!api) throw new Error('Not connected to chain')
  return api
}

function requireClient(): PolkadotClient {
  if (!client) throw new Error('Not connected to chain')
  return client
}

// ─────────────────────────────────────────────────────────────────────────────
// Chain properties
// ─────────────────────────────────────────────────────────────────────────────

export async function getChainProperties(): Promise<{
  tokenDecimals: number
  tokenSymbol: string
  blockTimeMs: number
  minProviderStake: bigint
  ss58Prefix: number
  specName: string
  specVersion: number
  genesisHash: string
}> {
  // Defaults match the current runtime; overridden where the chain exposes
  // them via constants / spec data.
  let tokenDecimals = 12
  let tokenSymbol = 'UNIT'
  let blockTimeMs = 6000
  let minProviderStake = 1_000_000_000_000_000n
  let ss58Prefix = getSs58Prefix()
  let specName = ''
  let specVersion = 0
  let genesisHash = ''

  if (client && api) {
    try {
      const spec = await client.getChainSpecData()
      genesisHash = spec.genesisHash || genesisHash
      const props = spec.properties as { tokenDecimals?: number | number[]; tokenSymbol?: string | string[]; ss58Format?: number } | undefined
      if (props) {
        const dec = props.tokenDecimals
        if (typeof dec === 'number') tokenDecimals = dec
        else if (Array.isArray(dec) && dec.length > 0) tokenDecimals = dec[0]
        const sym = props.tokenSymbol
        if (typeof sym === 'string') tokenSymbol = sym
        else if (Array.isArray(sym) && sym.length > 0) tokenSymbol = sym[0]
        if (typeof props.ss58Format === 'number') ss58Prefix = props.ss58Format
      }
    } catch { /* use defaults */ }

    // If chain spec didn't have ss58Format, try the runtime constant
    try {
      ss58Prefix = await api.constants.System.SS58Prefix()
    } catch { /* use default */ }

    try {
      const version = await api.constants.System.Version()
      specName = version.spec_name
      specVersion = version.spec_version
    } catch { /* use default */ }

    try {
      minProviderStake = await api.constants.StorageProvider.MinProviderStake()
    } catch { /* use default */ }

    try {
      const period = await api.constants.Aura.SlotDuration()
      blockTimeMs = Number(period)
    } catch { /* use default */ }
  }

  return { tokenDecimals, tokenSymbol, blockTimeMs, minProviderStake, ss58Prefix, specName, specVersion, genesisHash }
}

export async function getGenesisHash(): Promise<string> {
  const spec = await requireClient().getChainSpecData()
  return spec.genesisHash
}


// ─────────────────────────────────────────────────────────────────────────────
// Tx submission
// ─────────────────────────────────────────────────────────────────────────────

export type TxStatus =
  | { type: 'signing'; message: string }
  | { type: 'broadcast'; message: string }
  | { type: 'inBlock'; message: string; blockHash: string }
  | { type: 'finalized'; message: string; blockHash: string }
  | { type: 'error'; message: string }

export type TxProgressCallback = (status: TxStatus) => void

/**
 * Sign + submit a transaction, wait for finalization, throw on chain-side
 * failure or signing error. Streams progress through the optional callback.
 */
async function submit(
  tx: Transaction,
  signer: InjectedPolkadotAccount,
  description: string,
  onProgress?: TxProgressCallback,
): Promise<TxFinalizedPayload> {
  return new Promise((resolve, reject) => {
    let resolved = false
    const sub = tx.signSubmitAndWatch(signer.polkadotSigner).subscribe({
      next: (event) => {
        if (event.type === 'signed') {
          onProgress?.({ type: 'broadcast', message: 'Transaction broadcast to network…' })
        } else if (event.type === 'txBestBlocksState' && event.found) {
          onProgress?.({
            type: 'inBlock',
            message: `${description} included in block`,
            blockHash: event.block.hash,
          })
        } else if (event.type === 'finalized') {
          onProgress?.({
            type: 'finalized',
            message: `${description} finalized`,
            blockHash: event.block.hash,
          })
          if (event.ok) {
            if (!resolved) {
              resolved = true
              sub.unsubscribe()
              resolve(event)
            }
          } else {
            const err = JSON.stringify(event.dispatchError, (_, v) =>
              typeof v === 'bigint' ? v.toString() : v,
            )
            const message = `${description} failed on-chain: ${err}`
            onProgress?.({ type: 'error', message })
            if (!resolved) {
              resolved = true
              sub.unsubscribe()
              reject(new Error(message))
            }
          }
        }
      },
      error: (err) => {
        const message = err instanceof Error ? err.message : String(err)
        onProgress?.({ type: 'error', message })
        if (!resolved) {
          resolved = true
          reject(err instanceof Error ? err : new Error(message))
        }
      },
    })
  })
}

// ─────────────────────────────────────────────────────────────────────────────
// Public types (consumed by state/provider.state.ts)
// ─────────────────────────────────────────────────────────────────────────────

export interface OnChainProviderInfo {
  stake: bigint
  capacity?: bigint
  usedCapacity?: bigint
  activeBuckets: number
  registeredAt: number
  multiaddr?: string
  deregisterAt?: number
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
  chunkIndex: number
  mmrRoot: string
  startSeq: number
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
  lastCheckpointWindow: bigint | null
  checkpointPoolBalance: bigint
  checkpointReward: bigint
}

// ─────────────────────────────────────────────────────────────────────────────
// Storage queries
// ─────────────────────────────────────────────────────────────────────────────

export async function isProviderRegistered(address: string): Promise<boolean> {
  const provider = await requireApi().query.StorageProvider.Providers.getValue(address)
  return provider !== undefined
}

export async function getProviderData(
  address: string,
): Promise<{ info: OnChainProviderInfo; settings: OnChainProviderSettings } | null> {
  const provider = await requireApi().query.StorageProvider.Providers.getValue(address)
  if (!provider) return null

  const info: OnChainProviderInfo = {
    stake: provider.stake,
    capacity: provider.settings.max_capacity,
    usedCapacity: provider.committed_bytes,
    activeBuckets: provider.stats.agreements_total,
    registeredAt: provider.stats.registered_at,
    multiaddr: new TextDecoder().decode(provider.multiaddr),
    deregisterAt: provider.deregister_at ?? undefined,
  }
  const s = provider.settings
  const settings: OnChainProviderSettings = {
    minDuration: s.min_duration,
    maxDuration: s.max_duration,
    pricePerByte: s.price_per_byte,
    acceptingPrimary: s.accepting_primary,
    acceptingReplica: s.replica_sync_price !== undefined,
    replicaSyncPrice: s.replica_sync_price ?? null,
    acceptingExtensions: s.accepting_extensions,
    maxCapacity: s.max_capacity,
  }
  return { info, settings }
}

export async function getAccountBalance(address: string): Promise<{
  free: bigint
  reserved: bigint
  frozen: bigint
} | null> {
  const account = await requireApi().query.System.Account.getValue(address)
  if (!account) return null
  return {
    free: account.data.free,
    reserved: account.data.reserved,
    frozen: account.data.frozen,
  }
}

export async function getProviderAgreements(address: string): Promise<OnChainAgreement[]> {
  const entries = await requireApi().query.StorageProvider.StorageAgreements.getEntries()
  const currentBlock = blockNumber$.getValue() || 0
  const out: OnChainAgreement[] = []
  for (const { keyArgs, value } of entries) {
    const [bucketIdRaw, providerAddr] = keyArgs
    if (!isSameAddress(providerAddr, address)) continue
    const bucketId = Number(bucketIdRaw)
    const expiresAt = value.expires_at
    let status: 'active' | 'expired' | 'terminated' = 'active'
    if (value.extensions_blocked) status = 'terminated'
    else if (currentBlock > expiresAt && expiresAt > 0) status = 'expired'
    out.push({
      id: bucketId,
      bucketId,
      provider: providerAddr,
      user: value.owner,
      maxBytes: value.max_bytes,
      pricePerByte: value.price_per_byte,
      paymentLocked: value.payment_locked,
      startBlock: value.started_at,
      endBlock: expiresAt,
      isPrimary: value.role.type === 'Primary',
      status,
    })
  }
  return out
}

export async function getAgreementRequests(_address: string): Promise<OnChainAgreementRequest[]> {
  // const entries = await requireApi().query.StorageProvider.AgreementRequests.getEntries()
  // const out: OnChainAgreementRequest[] = []
  // for (const { keyArgs, value } of entries) {
  //   const [bucketIdRaw, providerAddr] = keyArgs
  //   if (!isSameAddress(providerAddr, address)) continue
  //   out.push({
  //     bucketId: Number(bucketIdRaw),
  //     requester: value.requester,
  //     maxBytes: value.max_bytes,
  //     paymentLocked: value.payment_locked,
  //     duration: value.duration,
  //     expiresAt: value.expires_at,
  //   })
  // }
  // return out
  return [];
}

export async function getProviderCheckpoints(address: string): Promise<OnChainCheckpoint[]> {
  const agreements = await getProviderAgreements(address)
  const bucketIds = [...new Set(agreements.map((a) => a.bucketId))]
  const out: OnChainCheckpoint[] = []
  for (const bucketId of bucketIds) {
    const bucket = await requireApi().query.StorageProvider.Buckets.getValue(BigInt(bucketId))
    if (!bucket || !bucket.snapshot) continue
    out.push({
      bucketId,
      mmrRoot: bucket.snapshot.mmr_root,
      leafCount: Number(bucket.snapshot.leaf_count),
      submittedAt: bucket.snapshot.checkpoint_block,
      blockNumber: bucket.snapshot.checkpoint_block,
      providers: bucket.primary_providers,
    })
  }
  return out
}

export async function getBucketDetails(
  bucketIds: number[],
  providerAddress: string,
): Promise<OnChainBucketDetails[]> {
  const a = requireApi()
  const out: OnChainBucketDetails[] = []
  for (const bucketId of bucketIds) {
    const bucket = await a.query.StorageProvider.Buckets.getValue(BigInt(bucketId))
    if (!bucket) continue

    const members: OnChainBucketMember[] = bucket.members.map((m) => ({
      account: m.account,
      role: m.role.type as 'Admin' | 'Writer' | 'Reader',
    }))

    const snapshot: OnChainBucketSnapshot | null = bucket.snapshot
      ? {
          mmrRoot: bucket.snapshot.mmr_root,
          startSeq: Number(bucket.snapshot.start_seq),
          leafCount: Number(bucket.snapshot.leaf_count),
          checkpointBlock: bucket.snapshot.checkpoint_block,
        }
      : null

    const historicalRoots: Array<{ position: number; root: string }> = []
    for (const [pos, root] of bucket.historical_roots) {
      const posNum = Number(pos)
      const rootStr = root
      if (posNum > 0 || (rootStr !== '0x' && !/^0x0+$/.test(rootStr))) {
        historicalRoots.push({ position: posNum, root: rootStr })
      }
    }

    let checkpointConfig: OnChainCheckpointConfig | null = null
    try {
      const config = await a.query.StorageProvider.CheckpointConfigs.getValue(BigInt(bucketId))
      if (config) {
        checkpointConfig = {
          interval: config.interval,
          gracePeriod: config.grace_period,
          enabled: config.enabled,
        }
      }
    } catch { /* not configured */ }

    let lastCheckpointWindow: bigint | null = null
    try {
      const val = await a.query.StorageProvider.LastCheckpointWindow.getValue(BigInt(bucketId))
      lastCheckpointWindow = val ?? null
    } catch { /* not set */ }

    let checkpointPoolBalance = 0n
    try {
      const pool = await a.query.StorageProvider.CheckpointPool.getValue(BigInt(bucketId))
      checkpointPoolBalance = pool ?? 0n
    } catch { /* no pool */ }

    let checkpointReward = 0n
    try {
      const reward = await a.query.StorageProvider.CheckpointRewards.getValue(
        providerAddress,
        BigInt(bucketId),
      )
      checkpointReward = reward ?? 0n
    } catch { /* no reward */ }

    out.push({
      bucketId,
      members,
      frozenStartSeq: bucket.frozen_start_seq != null ? Number(bucket.frozen_start_seq) : null,
      minProviders: bucket.min_providers,
      primaryProviders: bucket.primary_providers,
      snapshot,
      historicalRoots,
      totalSnapshots: bucket.total_snapshots,
      checkpointConfig,
      lastCheckpointWindow,
      checkpointPoolBalance,
      checkpointReward,
    })
  }
  return out
}

export async function getProviderChallenges(address: string): Promise<OnChainChallenge[]> {
  const entries = await requireApi().query.StorageProvider.Challenges.getEntries()
  const currentBlock = blockNumber$.getValue() || 0
  const challenges: OnChainChallenge[] = []
  for (const { keyArgs, value } of entries) {
    const deadline = Number(keyArgs[0])
    for (let idx = 0; idx < value.length; idx++) {
      const ch = value[idx]
      if (!isSameAddress(ch.provider, address)) continue
      challenges.push({
        id: idx,
        bucketId: Number(ch.bucket_id),
        challenger: ch.challenger,
        provider: ch.provider,
        leafIndex: Number(ch.leaf_index),
        chunkIndex: Number(ch.chunk_index),
        mmrRoot: ch.mmr_root,
        startSeq: Number(ch.start_seq),
        status: currentBlock > deadline ? 'expired' : 'pending',
        createdAt: 0,
        deadline,
      })
    }
  }
  return challenges.sort((a, b) => b.deadline - a.deadline)
}

// ─────────────────────────────────────────────────────────────────────────────
// Extrinsic submission
// ─────────────────────────────────────────────────────────────────────────────

export interface RegisterProviderParams {
  stake: bigint
  multiaddr: string
  publicKey?: Uint8Array
}

export async function submitRegisterProvider(
  params: RegisterProviderParams,
  settings: OnChainProviderSettings,
  signer: InjectedPolkadotAccount,
  onProgress?: TxProgressCallback,
): Promise<void> {
  const a = requireApi()
  const publicKey = params.publicKey || signer.polkadotSigner.publicKey
  if (!publicKey) throw new Error('No public key available for signing')

  const multiaddrBytes = new TextEncoder().encode(params.multiaddr)
  onProgress?.({ type: 'signing', message: 'Signing registration transaction...' })
  const tx = a.tx.StorageProvider.register_provider({
    multiaddr: multiaddrBytes,
    public_key: publicKey,
    stake: params.stake,
  })
  await submit(tx, signer, 'Register provider', onProgress)

  onProgress?.({ type: 'signing', message: 'Now updating provider settings...' })
  await submitUpdateSettings(settings, signer, onProgress)
}

export async function submitUpdateSettings(
  settings: OnChainProviderSettings,
  signer: InjectedPolkadotAccount,
  onProgress?: TxProgressCallback,
): Promise<void> {
  const a = requireApi()
  const tx = a.tx.StorageProvider.update_provider_settings({
    settings: {
      min_duration: settings.minDuration,
      max_duration: settings.maxDuration,
      price_per_byte: settings.pricePerByte,
      accepting_primary: settings.acceptingPrimary,
      replica_sync_price:
        settings.acceptingReplica && settings.replicaSyncPrice ? settings.replicaSyncPrice : undefined,
      accepting_extensions: settings.acceptingExtensions,
      max_capacity: settings.maxCapacity,
    },
  })
  await submit(tx, signer, 'Update settings', onProgress)
}

export async function submitUpdateMultiaddr(
  multiaddr: string,
  signer: InjectedPolkadotAccount,
  onProgress?: TxProgressCallback,
): Promise<void> {
  const a = requireApi()
  const multiaddrBytes = new TextEncoder().encode(multiaddr)
  const tx = a.tx.StorageProvider.update_provider_multiaddr({ multiaddr: multiaddrBytes })
  await submit(tx, signer, 'Update multiaddr', onProgress)
}

export async function submitAddStake(
  amount: bigint,
  signer: InjectedPolkadotAccount,
): Promise<void> {
  const a = requireApi()
  const tx = a.tx.StorageProvider.add_stake({ amount })
  await submit(tx, signer, 'Add stake')
}

export async function submitDeregisterProvider(
  signer: InjectedPolkadotAccount,
  onProgress?: TxProgressCallback,
): Promise<void> {
  const a = requireApi()
  onProgress?.({ type: 'signing', message: 'Signing de-registration announcement...' })
  const tx = a.tx.StorageProvider.deregister_provider()
  await submit(tx, signer, 'Deregister provider', onProgress)
}

export async function submitCompleteDeregister(
  signer: InjectedPolkadotAccount,
  onProgress?: TxProgressCallback,
): Promise<void> {
  const a = requireApi()
  onProgress?.({ type: 'signing', message: 'Signing de-registration completion...' })
  const tx = a.tx.StorageProvider.complete_deregister()
  await submit(tx, signer, 'Complete deregister', onProgress)
}

// submitChallengeResponse intentionally omitted — challenge response requires
// a structured `ChallengeResponse` payload (not just bytes) and a structured
// `ChallengeId { deadline, index }`. The Challenges UI doesn't currently
// invoke it; add back when implementing manual challenge response.

// ── Challenge proof fetching & response submission ──────────────────────────

export interface ChallengeProofData {
  chunkData: Uint8Array
  mmrProof: {
    peaks: string[]
    leaf: { dataRoot: string; dataSize: bigint; totalSize: bigint }
    leafProof: { siblings: string[]; path: boolean[] }
  }
  chunkProof: { siblings: string[]; path: boolean[] }
}

/**
 * Fetch MMR proof + chunk proof from the provider node HTTP API.
 */
export async function fetchChallengeProof(
  providerHttp: string,
  bucketId: number,
  leafIndex: number,
  chunkIndex: number,
): Promise<ChallengeProofData> {
  // Step 1: MMR proof
  const mmrRes = await fetch(
    `${providerHttp}/mmr_proof?bucket_id=${bucketId}&leaf_index=${leafIndex}`,
  )
  if (!mmrRes.ok) {
    throw new Error(`MMR proof fetch failed: ${mmrRes.status} ${await mmrRes.text()}`)
  }
  const mmr = await mmrRes.json()

  // Step 2: Chunk proof — provider hex_decode doesn't accept 0x prefix
  const dataRoot: string = mmr.leaf.data_root
  const dataRootBare = dataRoot.replace(/^0x/, '')
  const chunkRes = await fetch(
    `${providerHttp}/chunk_proof?data_root=${dataRootBare}&chunk_index=${chunkIndex}`,
  )
  if (!chunkRes.ok) {
    throw new Error(`Chunk proof fetch failed: ${chunkRes.status} ${await chunkRes.text()}`)
  }
  const chunk = await chunkRes.json()

  // Decode base64 chunk_data to Uint8Array
  const chunkData = chunk.chunk_data
    ? Uint8Array.from(atob(chunk.chunk_data), (c) => c.charCodeAt(0))
    : new Uint8Array(0)

  return {
    chunkData,
    mmrProof: {
      peaks: mmr.proof.peaks as string[],
      leaf: {
        dataRoot,
        dataSize: BigInt(mmr.leaf.data_size),
        totalSize: BigInt(mmr.leaf.total_size),
      },
      leafProof: {
        siblings: mmr.proof.siblings as string[],
        path: mmr.proof.path as boolean[],
      },
    },
    chunkProof: {
      siblings: chunk.proof.siblings as string[],
      path: chunk.proof.path as boolean[],
    },
  }
}

/**
 * Build and submit a `respond_to_challenge` extrinsic with a Proof response.
 */
export async function submitRespondToChallenge(
  challengeId: { deadline: number; index: number },
  proof: ChallengeProofData,
  signer: InjectedPolkadotAccount,
  onProgress?: TxProgressCallback,
): Promise<TxFinalizedPayload> {
  const a = requireApi()
  onProgress?.({ type: 'signing', message: 'Signing challenge response...' })

  const tx = a.tx.StorageProvider.respond_to_challenge({
    challenge_id: challengeId,
    response: Enum('Proof', {
      chunk_data: proof.chunkData,
      mmr_proof: {
        peaks: proof.mmrProof.peaks,
        leaf: {
          data_root: proof.mmrProof.leaf.dataRoot,
          data_size: proof.mmrProof.leaf.dataSize,
          total_size: proof.mmrProof.leaf.totalSize,
        },
        leaf_proof: {
          siblings: proof.mmrProof.leafProof.siblings,
          path: proof.mmrProof.leafProof.path,
        },
      },
      chunk_proof: {
        siblings: proof.chunkProof.siblings,
        path: proof.chunkProof.path,
      },
    }),
  })

  return submit(tx, signer, 'Challenge response', onProgress)
}

// ─────────────────────────────────────────────────────────────────────────────
// Subscriptions
// ─────────────────────────────────────────────────────────────────────────────

export function subscribeToBlocks(callback: (blockNumber: number) => void): () => void {
  if (!client) {
    console.warn('Cannot subscribe to blocks: not connected')
    return () => {}
  }
  const sub = client.finalizedBlock$.subscribe({
    next: (block) => callback(block.number),
    error: (err) => console.error('Block subscription error:', err),
  })
  return () => sub.unsubscribe()
}

export function subscribeToChallengeEvents(
  address: string,
  onChallenge: (challenge: OnChainChallenge) => void,
): () => void {
  if (!client || !api) {
    console.warn('Cannot subscribe to challenge events: not connected')
    return () => {}
  }
  const a = api
  const c = client
  const created = a.event.StorageProvider.ChallengeCreated.watch().subscribe({
    next: ({ block, events }) => {
      for (const { payload } of events) {
        if (!isSameAddress(payload.provider, address)) continue
        onChallenge({
          id: payload.challenge_id.index,
          bucketId: Number(payload.bucket_id),
          challenger: payload.challenger,
          provider: payload.provider,
          // leafIndex/chunkIndex aren't part of the ChallengeCreated event
          // payload — they live on the storage `Challenges` entry, not the
          // event. Default to 0 here; the Challenges page reads the storage
          // entry for the full record when rendering.
          leafIndex: 0,
          chunkIndex: 0,
          mmrRoot: '',
          startSeq: 0,
          status: 'pending',
          challengeType: 'unknown',
          createdAt: block.number,
          deadline: payload.challenge_id.deadline,
        })
      }
    },
    error: (err) => console.error('ChallengeCreated subscription error:', err),
  })
  const defended = a.event.StorageProvider.ChallengeDefended.watch().subscribe({
    next: ({ events }) => {
      for (const { payload } of events) {
        if (!isSameAddress(payload.provider, address)) continue
        onChallenge({
          id: Number(payload.challenge_id.index),
          bucketId: 0,
          challenger: '',
          provider: address,
          leafIndex: 0,
          chunkIndex: 0,
          mmrRoot: '',
          startSeq: 0,
          status: 'responded',
          createdAt: 0,
          deadline: Number(payload.challenge_id.deadline),
        })
      }
    },
    error: (err) => console.error('ChallengeDefended subscription error:', err),
  })
  const slashed = a.event.StorageProvider.ChallengeSlashed.watch().subscribe({
    next: ({ events }) => {
      for (const { payload } of events) {
        if (!isSameAddress(payload.provider, address)) continue
        onChallenge({
          id: Number(payload.challenge_id.index),
          bucketId: 0,
          challenger: '',
          provider: address,
          leafIndex: 0,
          chunkIndex: 0,
          mmrRoot: '',
          startSeq: 0,
          status: 'slashed',
          createdAt: 0,
          deadline: Number(payload.challenge_id.deadline),
        })
      }
    },
    error: (err) => console.error('ChallengeSlashed subscription error:', err),
  })
  // Avoid "unused" warning on c since the function captured it for closure-
  // visibility purposes in case future changes need raw client access.
  void c
  return () => {
    created.unsubscribe()
    defended.unsubscribe()
    slashed.unsubscribe()
  }
}
