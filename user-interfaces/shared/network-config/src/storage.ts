import type { NetworkId, NetworkConfig } from './types'
import { NETWORKS, DEFAULT_NETWORK_ID } from './networks'

const STORAGE_KEY = 'web3-storage-selected-network'
const CUSTOM_CONFIG_KEY = 'web3-storage-custom-network'

export interface PersistedNetwork {
  id: NetworkId
  customConfig?: { parachainWs: string; providerHttp: string }
}

export function saveSelectedNetwork(id: NetworkId, customConfig?: NetworkConfig): void {
  const data: PersistedNetwork = { id }
  if (id === 'custom' && customConfig) {
    data.customConfig = {
      parachainWs: customConfig.parachainWs,
      providerHttp: customConfig.providerHttp,
    }
    localStorage.setItem(CUSTOM_CONFIG_KEY, JSON.stringify(data.customConfig))
  }
  localStorage.setItem(STORAGE_KEY, id)
}

export function loadSelectedNetwork(): { id: NetworkId; config: NetworkConfig } {
  const savedId = localStorage.getItem(STORAGE_KEY) as NetworkId | null
  const id = savedId && savedId in NETWORKS ? savedId : DEFAULT_NETWORK_ID

  if (id === 'custom') {
    try {
      const raw = localStorage.getItem(CUSTOM_CONFIG_KEY)
      if (raw) {
        const parsed = JSON.parse(raw) as { parachainWs: string; providerHttp: string }
        return {
          id,
          config: {
            id: 'custom',
            name: 'Custom RPC',
            parachainWs: parsed.parachainWs,
            providerHttp: parsed.providerHttp,
            isTestnet: false,
          },
        }
      }
    } catch {
      // Fall through to default
    }
  }

  return { id, config: NETWORKS[id] }
}

export function clearSelectedNetwork(): void {
  localStorage.removeItem(STORAGE_KEY)
  localStorage.removeItem(CUSTOM_CONFIG_KEY)
}

/**
 * Pick up a network selection passed via URL (e.g. landing-page handoff).
 * Reads ?network=, optionally ?parachainWs=&providerHttp= for custom, persists
 * via saveSelectedNetwork(), then strips the params from the URL so a refresh
 * doesn't re-trigger the handoff. Returns true if a selection was applied.
 */
export function loadFromUrl(): boolean {
  if (typeof window === 'undefined') return false

  const params = new URLSearchParams(window.location.search)
  const id = params.get('network') as NetworkId | null
  if (!id || !(id in NETWORKS)) return false

  if (id === 'custom') {
    const parachainWs = params.get('parachainWs')
    const providerHttp = params.get('providerHttp')
    if (!parachainWs || !providerHttp) return false
    saveSelectedNetwork('custom', {
      id: 'custom',
      name: 'Custom RPC',
      parachainWs,
      providerHttp,
      isTestnet: false,
    })
  } else {
    saveSelectedNetwork(id)
  }

  params.delete('network')
  params.delete('parachainWs')
  params.delete('providerHttp')
  const qs = params.toString()
  const newUrl = window.location.pathname + (qs ? `?${qs}` : '') + window.location.hash
  window.history.replaceState({}, '', newUrl)
  return true
}
