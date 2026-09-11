// SPDX-License-Identifier: GPL-3.0-only

/**
 * Network State — network selection and persistence.
 *
 * Manages which blockchain network the app connects to. Persists the selection
 * to localStorage so it survives page reloads.
 */

import { BehaviorSubject } from 'rxjs'
import { bind } from '@react-rxjs/core'
import {
  type NetworkId,
  type NetworkConfig,
  NETWORKS,
  NETWORK_LIST,
  loadSelectedNetwork,
  saveSelectedNetwork,
  createCustomNetwork,
} from '@web3-storage/network-config'
import { disconnect, connect } from '@/state/chain.state'

const persisted = loadSelectedNetwork()

const selectedNetworkId$ = new BehaviorSubject<NetworkId>(persisted.id)
const selectedNetwork$ = new BehaviorSubject<NetworkConfig>(persisted.config)

export const [useSelectedNetwork] = bind(selectedNetwork$, persisted.config)
export const [useNetworkList] = bind(new BehaviorSubject(NETWORK_LIST), NETWORK_LIST)

// ─────────────────────────────────────────────────────────────────────────────
// Actions
// ─────────────────────────────────────────────────────────────────────────────

export async function selectNetwork(id: NetworkId): Promise<void> {
  const config = NETWORKS[id]
  if (!config) return

  selectedNetworkId$.next(id)
  selectedNetwork$.next(config)
  saveSelectedNetwork(id)

  disconnect()
  await connect(config.parachainWs)
}

export async function selectCustomNetwork(input: {
  parachainWs: string
  providerHttp: string
}): Promise<void> {
  const config = createCustomNetwork(input.parachainWs, input.providerHttp)

  selectedNetworkId$.next('custom')
  selectedNetwork$.next(config)
  saveSelectedNetwork('custom', config)

  disconnect()
  await connect(config.parachainWs)
}

// ─────────────────────────────────────────────────────────────────────────────
// Getters (non-reactive)
// ─────────────────────────────────────────────────────────────────────────────

export function getParachainWs(): string {
  return selectedNetwork$.getValue().parachainWs
}
