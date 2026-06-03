export type { NetworkId, NetworkConfig } from './types'
export {
  LOCAL_NETWORK,
  WESTEND_NETWORK,
  PASEO_NETWORK,
  POLKADOT_NETWORK,
  NETWORKS,
  NETWORK_LIST,
  DEFAULT_NETWORK_ID,
  createCustomNetwork,
} from './networks'
export type { PersistedNetwork } from './storage'
export {
  saveSelectedNetwork,
  loadSelectedNetwork,
  clearSelectedNetwork,
  loadFromUrl,
  withNetworkHandoff,
} from './storage'

import { loadFromUrl as _loadFromUrl } from './storage'
// Apply ?network=... handoff from the landing page before any consumer reads
// localStorage. This module evaluates before importers (ESM depth-first), so
// state files that call loadSelectedNetwork() at top level see the fresh value.
if (typeof window !== 'undefined') {
  _loadFromUrl()
}
