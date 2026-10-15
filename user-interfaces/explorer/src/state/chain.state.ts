// SPDX-License-Identifier: GPL-3.0-only

/**
 * Chain State - Blockchain connection and block tracking
 *
 * Manages the WebSocket connection to the blockchain and the two clocks the
 * explorer renders against: the parachain height (display only) and the
 * pallet's anchor block (every on-chain duration).
 */

import { BehaviorSubject, map } from 'rxjs'
import { bind } from '@react-rxjs/core'
import {
  anchorBlock$,
  connectToChain,
  disconnectFromChain,
  refreshAnchorBlock,
  subscribeToBlocks,
  getChainProperties,
} from '@/lib/chain-client'
import { configureFormat } from '@web3-storage/format'
import { loadSelectedNetwork } from '@web3-storage/network-config'
import { updateSs58Prefix } from '@/state/wallet.state'

// Types
export type ConnectionStatus = 'disconnected' | 'connecting' | 'connected' | 'error'

export interface ChainInfo {
  name: string
  version: string
  genesisHash: string
}

// State subjects
const connectionStatus$ = new BehaviorSubject<ConnectionStatus>('disconnected')
const blockNumber$ = new BehaviorSubject<number>(0)
const chainInfo$ = new BehaviorSubject<ChainInfo | null>(null)
const initialNetwork = loadSelectedNetwork()
const endpoint$ = new BehaviorSubject<string>(initialNetwork.config.parachainWs)
const connectionError$ = new BehaviorSubject<string | undefined>(undefined)

// Block subscription cleanup
let blockUnsubscribe: (() => void) | null = null

// React hooks
export const [useConnectionStatus] = bind(connectionStatus$, 'disconnected')
export const [useBlockNumber] = bind(blockNumber$, 0)
/**
 * The pallet's anchor block — use this (never `useBlockNumber`) for anything
 * compared against on-chain durations: agreement expiry, challenge deadlines.
 */
export const [useAnchorBlock] = bind(anchorBlock$.pipe(map((n) => n ?? 0)), 0)
export const [useChainInfo] = bind(chainInfo$, null)
export const [useConnectionError] = bind(connectionError$, undefined)

export const [useIsConnected] = bind(
  connectionStatus$.pipe(map((status) => status === 'connected')),
  false
)

// ─────────────────────────────────────────────────────────────────────────────
// Actions
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Connect to the blockchain (read-only: queries, blocks, runtime APIs).
 */
export async function connect(wsEndpoint?: string): Promise<void> {
  const ep = wsEndpoint || endpoint$.getValue()
  // StrictMode double-mounts the boot effect; a second connect to the same
  // endpoint would orphan a block subscription and double the anchor polling.
  const status = connectionStatus$.getValue()
  if ((status === 'connecting' || status === 'connected') && endpoint$.getValue() === ep) {
    return
  }
  endpoint$.next(ep)
  connectionStatus$.next('connecting')
  connectionError$.next(undefined)

  try {
    await connectToChain(ep)

    // Fetch chain properties and apply all chain-derived config: number
    // formatting, the SS58 prefix, and the chain identity.
    const chainProps = await getChainProperties()
    configureFormat(chainProps)
    await updateSs58Prefix(chainProps.ss58Prefix)

    chainInfo$.next({
      name: chainProps.specName,
      version: String(chainProps.specVersion),
      genesisHash: chainProps.genesisHash,
    })

    connectionStatus$.next('connected')

    // Seed the anchor clock immediately rather than waiting for the first
    // finalized block — otherwise every page briefly renders against anchor 0
    // and over-counts "active" agreements. This queries the runtime API; on a
    // pre-anchor runtime it falls back to the 0 passed in, i.e. exactly the
    // unseeded state, so nothing is fabricated.
    void refreshAnchorBlock(0)

    // Subscribe to blocks; each finalized block also refreshes the pallet's
    // anchor clock (a cheap runtime API call).
    if (blockUnsubscribe) {
      blockUnsubscribe()
      blockUnsubscribe = null
    }
    blockUnsubscribe = subscribeToBlocks((block) => {
      blockNumber$.next(block)
      void refreshAnchorBlock(block)
    })

    // Show a provisional #1 only if the subscription hasn't already
    // delivered the real height.
    if (blockNumber$.getValue() === 0) {
      blockNumber$.next(1)
    }
  } catch (error) {
    connectionStatus$.next('error')
    connectionError$.next(error instanceof Error ? error.message : 'Connection failed')
    throw error
  }
}

/**
 * Disconnect from the blockchain
 */
export function disconnect(): void {
  if (blockUnsubscribe) {
    blockUnsubscribe()
    blockUnsubscribe = null
  }

  disconnectFromChain()
  connectionStatus$.next('disconnected')
  blockNumber$.next(0)
  chainInfo$.next(null)
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilities
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Check if connected (non-reactive)
 */
export function isConnected(): boolean {
  return connectionStatus$.getValue() === 'connected'
}

/**
 * Get the pallet's anchor block (non-reactive) — the clock all on-chain
 * durations are measured against. See [`useAnchorBlock`].
 */
export function getAnchorBlock(): number {
  return anchorBlock$.getValue() ?? 0
}
