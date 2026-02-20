import { BehaviorSubject, interval, switchMap, catchError, of, map, distinctUntilChanged, shareReplay } from 'rxjs'
import { bind } from '@react-rxjs/core'
import { createSignal } from '@react-rxjs/utils'

// Types
export type ConnectionStatus = 'disconnected' | 'connecting' | 'connected' | 'error'

export interface ChainState {
  endpoint: string
  status: ConnectionStatus
  blockNumber: number
  chainName: string
  error?: string
}

// Signals for actions
const [endpointChange$, setEndpoint] = createSignal<string>()
const [connectionTrigger$, triggerConnection] = createSignal<void>()

// State subjects
const connectionStatus$ = new BehaviorSubject<ConnectionStatus>('disconnected')
const blockNumber$ = new BehaviorSubject<number>(0)
const chainName$ = new BehaviorSubject<string>('')
const endpoint$ = new BehaviorSubject<string>('ws://127.0.0.1:9944')
const connectionError$ = new BehaviorSubject<string | undefined>(undefined)

// Block polling when connected
const blockPolling$ = connectionStatus$.pipe(
  switchMap((status) => {
    if (status !== 'connected') {
      return of(null)
    }
    // Poll every 6 seconds (block time)
    return interval(6000).pipe(
      switchMap(async () => {
        // In a real implementation, this would query the chain
        // For now, simulate block increment
        const current = blockNumber$.getValue()
        return current + 1
      }),
      catchError((err) => {
        console.error('Block polling error:', err)
        return of(null)
      })
    )
  }),
  shareReplay(1)
)

// Subscribe to block polling to update state
blockPolling$.subscribe((block) => {
  if (block !== null) {
    blockNumber$.next(block)
  }
})

// React hooks
export const [useConnectionStatus] = bind(connectionStatus$, 'disconnected')
export const [useBlockNumber] = bind(blockNumber$, 0)
export const [useChainName] = bind(chainName$, '')
export const [useEndpoint] = bind(endpoint$, 'ws://127.0.0.1:9944')
export const [useConnectionError] = bind(connectionError$, undefined)

export const [useIsConnected] = bind(
  connectionStatus$.pipe(map((status) => status === 'connected')),
  false
)

// Actions
export async function connect(wsEndpoint?: string): Promise<void> {
  const ep = wsEndpoint || endpoint$.getValue()
  endpoint$.next(ep)
  connectionStatus$.next('connecting')
  connectionError$.next(undefined)

  try {
    // TODO: Implement actual polkadot-api connection
    // For now, simulate connection
    await new Promise((resolve) => setTimeout(resolve, 1000))

    connectionStatus$.next('connected')
    chainName$.next('Storage Parachain')
    blockNumber$.next(1)
  } catch (error) {
    connectionStatus$.next('error')
    connectionError$.next(error instanceof Error ? error.message : 'Connection failed')
    throw error
  }
}

export function disconnect(): void {
  connectionStatus$.next('disconnected')
  blockNumber$.next(0)
  chainName$.next('')
}

// Export state setters for testing/mocking
export const chainActions = {
  connect,
  disconnect,
  setEndpoint: (ep: string) => endpoint$.next(ep),
  setBlockNumber: (block: number) => blockNumber$.next(block),
  setConnectionStatus: (status: ConnectionStatus) => connectionStatus$.next(status),
}
