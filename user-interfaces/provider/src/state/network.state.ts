// SPDX-License-Identifier: Apache-2.0

/**
 * Network State - Network selection and persistence
 *
 * Manages which blockchain network the dashboard connects to.
 * Persists selection to localStorage so it survives page reloads.
 */

import { BehaviorSubject, map } from 'rxjs'
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

// Initialize from localStorage
const persisted = loadSelectedNetwork()

const selectedNetworkId$ = new BehaviorSubject<NetworkId>(persisted.id)
const selectedNetwork$ = new BehaviorSubject<NetworkConfig>(persisted.config)

// React hooks
export const [useSelectedNetworkId] = bind(selectedNetworkId$, persisted.id)
export const [useSelectedNetwork] = bind(selectedNetwork$, persisted.config)

export const [useParachainWs] = bind(
  selectedNetwork$.pipe(map((n) => n.parachainWs)),
  persisted.config.parachainWs
)

export const [useProviderHttp] = bind(
  selectedNetwork$.pipe(map((n) => n.providerHttp)),
  persisted.config.providerHttp
)

export const [useIsTestnet] = bind(
  selectedNetwork$.pipe(map((n) => n.isTestnet)),
  persisted.config.isTestnet
)

export const [useNetworkList] = bind(
  new BehaviorSubject(NETWORK_LIST),
  NETWORK_LIST
)

// ─────────────────────────────────────────────────────────────────────────────
// Actions
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Switch to a predefined network by ID.
 * Disconnects from current chain and reconnects to the new endpoint.
 */
export async function selectNetwork(id: NetworkId): Promise<void> {
  const config = NETWORKS[id]
  if (!config) return

  selectedNetworkId$.next(id)
  selectedNetwork$.next(config)
  saveSelectedNetwork(id)

  // Reconnect to new endpoint
  disconnect()
  await connect(config.parachainWs)
}

/**
 * Switch to a custom network with user-provided endpoints.
 */
export async function selectCustomNetwork(input: {
  parachainWs: string
  providerHttp: string
}): Promise<void> {
  const config = createCustomNetwork(input.parachainWs, input.providerHttp)

  selectedNetworkId$.next('custom')
  selectedNetwork$.next(config)
  saveSelectedNetwork('custom', config)

  // Reconnect to new endpoint
  disconnect()
  await connect(config.parachainWs)
}

// ─────────────────────────────────────────────────────────────────────────────
// Getters (non-reactive)
// ─────────────────────────────────────────────────────────────────────────────

export function getSelectedNetwork(): NetworkConfig {
  return selectedNetwork$.getValue()
}

export function getParachainWs(): string {
  return selectedNetwork$.getValue().parachainWs
}

export function getProviderHttp(): string {
  return selectedNetwork$.getValue().providerHttp
}
