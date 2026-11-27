// SPDX-License-Identifier: GPL-3.0-only

/**
 * Chain State — the storage parachain's connection through the TrUAPI host,
 * its runtime, and its best and finalized blocks.
 */

import type { HexString } from '@parity/truapi'
import { BehaviorSubject, type Subscription } from 'rxjs'
import { bind } from '@react-rxjs/core'
import { connectToChain, disconnectFromChain, getChainProperties } from '@/lib/chain-client'
import { configureFromChain } from '@/utils/format'
import { isInHost } from '@/lib/host-account'
import { createHostChainProvider } from '@/lib/host-chain-provider'
import { recordArrival, type BlockArrival } from '@/lib/chain-health'

export type ConnectionStatus = 'disconnected' | 'connecting' | 'connected' | 'error'

export interface RuntimeInfo {
  specName: string
  specVersion: number
}

/** The name the shared chain client gives the host-routed connection. */
const HOST_CONNECTION = 'truapi-host'

const connectionStatus$ = new BehaviorSubject<ConnectionStatus>('disconnected')
const connectionError$ = new BehaviorSubject<string | undefined>(undefined)
const runtime$ = new BehaviorSubject<RuntimeInfo | undefined>(undefined)
const finalizedNumber$ = new BehaviorSubject<number>(0)
const bestHistory$ = new BehaviorSubject<BlockArrival[]>([])

let subscriptions: Subscription[] = []

export const [useConnectionStatus] = bind(connectionStatus$, 'disconnected')
export const [useConnectionError] = bind(connectionError$, undefined)
export const [useRuntime] = bind(runtime$, undefined)
export const [useFinalizedNumber] = bind(finalizedNumber$, 0)
/** The latest best blocks with their arrival times, oldest first. */
export const [useBestHistory] = bind(bestHistory$, [])

// ─────────────────────────────────────────────────────────────────────────────
// Actions
// ─────────────────────────────────────────────────────────────────────────────

/** Connect to the parachain through the host. Does nothing outside a host. */
export async function connect(): Promise<void> {
  if (!isInHost()) return
  connectionStatus$.next('connecting')
  connectionError$.next(undefined)

  try {
    const client = await connectToChain(HOST_CONNECTION, createHostChainProvider(requireParachainGenesis()))

    // Apply chain-derived config (SS58 prefix, token format) before serving as connected.
    const [chainProps, specData] = await Promise.all([getChainProperties(), client.getChainSpecData().catch(() => undefined)])
    configureFromChain({ ss58Prefix: chainProps.ss58Prefix, specProperties: specData?.properties })
    runtime$.next({ specName: chainProps.specName, specVersion: chainProps.specVersion })

    connectionStatus$.next('connected')

    // Drop any previous subscriptions or they stack across reconnects.
    unsubscribe()
    subscriptions = [
      client.finalizedBlock$.subscribe({
        next: (block) => finalizedNumber$.next(block.number),
        error: (err) => fail(err),
      }),
      client.bestBlocks$.subscribe({
        next: ([best]) => {
          if (best) bestHistory$.next(recordArrival(bestHistory$.getValue(), best.number, Date.now()))
        },
        error: (err) => fail(err),
      }),
    ]
  } catch (error) {
    fail(error)
    throw error
  }
}

/**
 * The parachain's genesis hash, which names it to the host. The host's
 * `getChainInfo` resolves only the Trinity chains, so photos is told.
 */
function requireParachainGenesis(): HexString {
  const genesis = import.meta.env.VITE_PHOTOS_PARACHAIN_GENESIS
  if (!genesis?.startsWith('0x')) throw new Error('Set VITE_PHOTOS_PARACHAIN_GENESIS to reach the chain through the host')
  return genesis as HexString
}

export function disconnect(): void {
  unsubscribe()
  disconnectFromChain()
  connectionStatus$.next('disconnected')
  runtime$.next(undefined)
  finalizedNumber$.next(0)
  bestHistory$.next([])
}

function unsubscribe(): void {
  for (const subscription of subscriptions) subscription.unsubscribe()
  subscriptions = []
}

function fail(error: unknown): void {
  connectionStatus$.next('error')
  connectionError$.next(error instanceof Error ? error.message : 'Connection failed')
}
