// SPDX-License-Identifier: GPL-3.0-only

/**
 * Explorer State — the network-wide snapshot every page renders from.
 *
 * One snapshot for the whole app, reloaded on connect / network switch /
 * refresh tick. Nothing is persisted: a fresh page always shows fresh chain
 * state.
 */

import { BehaviorSubject } from 'rxjs'
import { bind } from '@react-rxjs/core'
import { loadNetworkSnapshot, type NetworkSnapshot } from '@/lib/explorer-client'
import { isConnected } from '@/state/chain.state'

const snapshot$ = new BehaviorSubject<NetworkSnapshot | null>(null)
const loading$ = new BehaviorSubject<boolean>(false)
const error$ = new BehaviorSubject<string | undefined>(undefined)

// Monotonic guard: only the newest in-flight load may publish. A slow query
// resolving after a network switch (or a StrictMode duplicate) must not
// clobber the newer network's snapshot.
let loadSeq = 0

export const [useSnapshot] = bind(snapshot$, null)
export const [useExplorerLoading] = bind(loading$, false)
export const [useExplorerError] = bind(error$, undefined)

/**
 * Drop the current snapshot and invalidate every in-flight load. Called on
 * network switch so the old network's data can't linger on screen (or be
 * published late by a load that started before the switch).
 */
export function resetSnapshot(): void {
  loadSeq++
  snapshot$.next(null)
  error$.next(undefined)
  loading$.next(false)
}

/**
 * Reload the snapshot. Non-silent loads clear the current snapshot first (a
 * full-page reload UX — used on connect and network switch); silent loads
 * refresh in the background without flashing placeholders.
 */
export async function loadAll(opts?: { silent?: boolean }): Promise<void> {
  if (!isConnected()) return
  const seq = ++loadSeq

  if (!opts?.silent) {
    snapshot$.next(null)
    loading$.next(true)
  }

  try {
    const snapshot = await loadNetworkSnapshot()
    if (seq !== loadSeq) return
    snapshot$.next(snapshot)
    error$.next(undefined)
  } catch (err) {
    if (seq !== loadSeq) return
    error$.next(err instanceof Error ? err.message : 'Failed to load network state')
  } finally {
    if (seq === loadSeq) loading$.next(false)
  }
}
