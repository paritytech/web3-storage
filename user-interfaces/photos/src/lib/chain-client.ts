// SPDX-License-Identifier: GPL-3.0-only

/**
 * Chain Client — direct blockchain interaction via WebSocket.
 *
 * Single-library setup: polkadot-api (PAPI) v2 typed API, built from the
 * Revive-inclusive workspace descriptors (`@polkadot-api/descriptors`). For the
 * M4 skeleton the only chain interaction is an unsigned `ReviveApi.call` read of
 * the Photos contract's `libraryOf` (see `lib/photos-contract.ts`), so this
 * client just manages the connection lifecycle, chain properties, and blocks.
 */

import { createClient, type PolkadotClient } from 'polkadot-api'
import { getWsProvider } from 'polkadot-api/ws'
import { parachain } from '@polkadot-api/descriptors'
import { BehaviorSubject } from 'rxjs'
import { getSs58Prefix, type ParachainApi } from '@web3-storage/papi'

export type { PolkadotClient }

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

/** The typed API, or throw if not connected. Consumed by the contract read. */
export function getApi(): ParachainApi {
  if (!api) throw new Error('Not connected to chain')
  return api
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
    try {
      const spec = await client.getChainSpecData()
      genesisHash = spec.genesisHash || genesisHash
      const props = spec.properties as { ss58Format?: number } | undefined
      if (props && typeof props.ss58Format === 'number') ss58Prefix = props.ss58Format
    } catch { /* use defaults */ }

    // If the chain spec didn't carry ss58Format, fall back to the runtime constant.
    try {
      ss58Prefix = await api.constants.System.SS58Prefix()
    } catch { /* use default */ }

    try {
      const version = await api.constants.System.Version()
      specName = version.spec_name
      specVersion = version.spec_version
    } catch { /* use default */ }
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
