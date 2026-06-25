// SPDX-License-Identifier: GPL-3.0-only

/**
 * Chain State — blockchain connection and block tracking.
 */

import { BehaviorSubject } from 'rxjs'
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

const connectionStatus$ = new BehaviorSubject<ConnectionStatus>('disconnected')
const blockNumber$ = new BehaviorSubject<number>(0)
const initialNetwork = loadSelectedNetwork()
const endpoint$ = new BehaviorSubject<string>(initialNetwork.config.parachainWs)
const connectionError$ = new BehaviorSubject<string | undefined>(undefined)

let blockUnsubscribe: (() => void) | null = null

export const [useConnectionStatus] = bind(connectionStatus$, 'disconnected')
export const [useBlockNumber] = bind(blockNumber$, 0)
export const [useConnectionError] = bind(connectionError$, undefined)

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

    // Apply chain-derived config (the SS58 prefix) before serving as connected.
    const chainProps = await getChainProperties()
    await configureFromChain(chainProps)

    connectionStatus$.next('connected')

    // finalizedBlock$ replays the current head into blockNumber$ on subscribe,
    // so the badge shows the real height immediately — no manual seed needed.
    blockUnsubscribe = subscribeToBlocks((block) => {
      blockNumber$.next(block)
    })
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
}
