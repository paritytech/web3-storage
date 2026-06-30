// SPDX-License-Identifier: GPL-3.0-only

/**
 * Shared chain-client — the generic chain connection lifecycle (PAPI v2 typed
 * API over WebSocket) reused by the UIs. Owns the per-app singleton connection
 * and exposes only the generic primitives: connect/disconnect, client/api
 * accessors, block subscription, and common chain properties. Each app's own
 * `lib/chain-client.ts` keeps its domain queries and tx submission.
 */

import { createClient, type PolkadotClient } from 'polkadot-api'
import { getWsProvider } from 'polkadot-api/ws'
import { parachain } from '@polkadot-api/descriptors'
import { BehaviorSubject } from 'rxjs'
import { getSs58Prefix, type ParachainApi } from '@web3-storage/papi'

export type { PolkadotClient, ParachainApi }

// ─────────────────────────────────────────────────────────────────────────────
// Connection state
// ─────────────────────────────────────────────────────────────────────────────

let client: PolkadotClient | null = null
let api: ParachainApi | null = null
let currentEndpoint: string = ''

export const clientReady$ = new BehaviorSubject<boolean>(false)

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
}

export function getClient(): PolkadotClient | null {
  return client
}

/** The typed API, or throw if not connected. */
export function requireApi(): ParachainApi {
  if (!api) throw new Error('Not connected to chain')
  return api
}

/** The client, or throw if not connected. */
export function requireClient(): PolkadotClient {
  if (!client) throw new Error('Not connected to chain')
  return client
}

// ─────────────────────────────────────────────────────────────────────────────
// Chain properties
// ─────────────────────────────────────────────────────────────────────────────

export async function getChainProperties(): Promise<{
  ss58Prefix: number
  specName: string
  specVersion: number
  genesisHash: string
}> {
  let ss58Prefix = getSs58Prefix()
  let specName = ''
  let specVersion = 0
  let genesisHash = ''

  if (client && api) {
    const c = client
    const a = api
    // Independent reads — run concurrently. The SS58 prefix comes from the
    // runtime constant (authoritative on this parachain); the chain spec is
    // read only for the genesis hash. allSettled keeps each field's default
    // on a per-read failure.
    const [spec, prefix, version] = await Promise.allSettled([
      c.getChainSpecData(),
      a.constants.System.SS58Prefix(),
      a.constants.System.Version(),
    ])
    if (spec.status === 'fulfilled') genesisHash = spec.value.genesisHash || genesisHash
    if (prefix.status === 'fulfilled') ss58Prefix = prefix.value
    if (version.status === 'fulfilled') {
      specName = version.value.spec_name
      specVersion = version.value.spec_version
    }
  }

  return { ss58Prefix, specName, specVersion, genesisHash }
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
