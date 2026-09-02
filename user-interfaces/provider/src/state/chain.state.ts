// SPDX-License-Identifier: GPL-3.0-only

/**
 * Chain State - Blockchain connection and block tracking
 *
 * This state manages the WebSocket connection to the blockchain,
 * independent of any provider node. New providers use this to
 * register before they even have a provider node running.
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
import { configureFromChain } from '@/utils/format'
import { loadSelectedNetwork } from '@web3-storage/network-config'

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
 * compared against on-chain durations: deregister cooldown, agreement
 * expiry/progress, challenge deadlines, checkpoint windows.
 */
export const [useAnchorBlock] = bind(anchorBlock$.pipe(map((n) => n ?? 0)), 0)
export const [useChainInfo] = bind(chainInfo$, null)
export const [useEndpoint] = bind(endpoint$, initialNetwork.config.parachainWs)
export const [useConnectionError] = bind(connectionError$, undefined)

export const [useIsConnected] = bind(
  connectionStatus$.pipe(map((status) => status === 'connected')),
  false
)

export const [useChainName] = bind(
  chainInfo$.pipe(map((info) => info?.name ?? '')),
  ''
)

// ─────────────────────────────────────────────────────────────────────────────
// Actions
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Connect to the blockchain
 *
 * This establishes a WebSocket connection to the chain, allowing:
 * - New providers to register (no provider node needed)
 * - Querying on-chain state
 * - Subscribing to blocks and events
 */
export async function connect(wsEndpoint?: string): Promise<void> {
  const ep = wsEndpoint || endpoint$.getValue()
  endpoint$.next(ep)
  connectionStatus$.next('connecting')
  connectionError$.next(undefined)

  try {
    await connectToChain(ep)

    // Fetch chain properties and apply all chain-derived config: formatting,
    // SS58 prefix, and minProviderStake. Returns the chain identity to publish.
    const chainProps = await getChainProperties()
    const info = await configureFromChain(chainProps)

    chainInfo$.next(info)

    connectionStatus$.next('connected')

    // Subscribe to blocks; each finalized block also refreshes the pallet's
    // anchor clock (a cheap runtime API call).
    blockUnsubscribe = subscribeToBlocks((block) => {
      blockNumber$.next(block)
      void refreshAnchorBlock(block)
    })

    // Set initial block. The anchor is deliberately NOT seeded here: its
    // pre-anchor-runtime fallback is the parachain height passed in, and a
    // fabricated `1` would be published as a real anchor value. It stays 0
    // (the safe direction — nothing shows as expired) until the first
    // finalized block refreshes it.
    blockNumber$.next(1)
    void refreshAnchorBlock(1)
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

/**
 * Reconnect with a new endpoint
 */
export async function reconnect(newEndpoint: string): Promise<void> {
  disconnect()
  await connect(newEndpoint)
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilities
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Get current connection status (non-reactive)
 */
export function getConnectionStatus(): ConnectionStatus {
  return connectionStatus$.getValue()
}

/**
 * Check if connected (non-reactive)
 */
export function isConnected(): boolean {
  return connectionStatus$.getValue() === 'connected'
}

/**
 * Get current block number (non-reactive)
 */
export function getCurrentBlock(): number {
  return blockNumber$.getValue()
}

/**
 * Get the pallet's anchor block (non-reactive) — the clock all on-chain
 * durations are measured against. See [`useAnchorBlock`].
 */
export function getAnchorBlock(): number {
  return anchorBlock$.getValue() ?? 0
}

// Export for testing
export const chainActions = {
  connect,
  disconnect,
  reconnect,
  setEndpoint: (ep: string) => endpoint$.next(ep),
  setBlockNumber: (block: number) => blockNumber$.next(block),
  setAnchorBlock: (block: number) => anchorBlock$.next(block),
  setConnectionStatus: (status: ConnectionStatus) => connectionStatus$.next(status),
}
