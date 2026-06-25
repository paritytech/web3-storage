// SPDX-License-Identifier: GPL-3.0-only

/**
 * Chain State — blockchain connection and block tracking.
 */

import { BehaviorSubject, map } from 'rxjs'
import { bind } from '@react-rxjs/core'
import {
  connectToChain,
  disconnectFromChain,
  subscribeToBlocks,
  getChainProperties,
} from '@/lib/chain-client'
import { configureFromChain } from '@/utils/format'
import { loadSelectedNetwork } from '@web3-storage/network-config'

export type ConnectionStatus = 'disconnected' | 'connecting' | 'connected' | 'error'

export interface ChainInfo {
  name: string
  version: string
  genesisHash: string
}

const connectionStatus$ = new BehaviorSubject<ConnectionStatus>('disconnected')
const blockNumber$ = new BehaviorSubject<number>(0)
const chainInfo$ = new BehaviorSubject<ChainInfo | null>(null)
const initialNetwork = loadSelectedNetwork()
const endpoint$ = new BehaviorSubject<string>(initialNetwork.config.parachainWs)
const connectionError$ = new BehaviorSubject<string | undefined>(undefined)

let blockUnsubscribe: (() => void) | null = null

export const [useConnectionStatus] = bind(connectionStatus$, 'disconnected')
export const [useBlockNumber] = bind(blockNumber$, 0)
export const [useChainInfo] = bind(chainInfo$, null)
export const [useEndpoint] = bind(endpoint$, initialNetwork.config.parachainWs)
export const [useConnectionError] = bind(connectionError$, undefined)

export const [useIsConnected] = bind(
  connectionStatus$.pipe(map((status) => status === 'connected')),
  false
)

// ─────────────────────────────────────────────────────────────────────────────
// Actions
// ─────────────────────────────────────────────────────────────────────────────

export async function connect(wsEndpoint?: string): Promise<void> {
  const ep = wsEndpoint || endpoint$.getValue()
  endpoint$.next(ep)
  connectionStatus$.next('connecting')
  connectionError$.next(undefined)

  try {
    await connectToChain(ep)

    // Fetch chain properties and apply chain-derived config (the SS58 prefix).
    // Returns the chain identity to publish into state.
    const chainProps = await getChainProperties()
    const info = await configureFromChain(chainProps)
    chainInfo$.next(info)

    connectionStatus$.next('connected')

    blockUnsubscribe = subscribeToBlocks((block) => {
      blockNumber$.next(block)
    })
    blockNumber$.next(1)
  } catch (error) {
    connectionStatus$.next('error')
    connectionError$.next(error instanceof Error ? error.message : 'Connection failed')
    throw error
  }
}

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

export async function reconnect(newEndpoint: string): Promise<void> {
  disconnect()
  await connect(newEndpoint)
}

// ─────────────────────────────────────────────────────────────────────────────
// Getters (non-reactive)
// ─────────────────────────────────────────────────────────────────────────────

export function getConnectionStatus(): ConnectionStatus {
  return connectionStatus$.getValue()
}

export function isConnected(): boolean {
  return connectionStatus$.getValue() === 'connected'
}
