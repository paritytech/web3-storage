import type { NetworkId, NetworkConfig } from './types'

export const LOCAL_NETWORK: NetworkConfig = {
  id: 'local',
  name: 'Local Development',
  parachainWs: 'ws://127.0.0.1:2222',
  providerHttp: 'http://127.0.0.1:3333',
  relayChainWs: 'ws://127.0.0.1:9900',
  isTestnet: true,
}

export const WESTEND_NETWORK: NetworkConfig = {
  id: 'westend',
  name: 'Westend Testnet',
  parachainWs: '',
  providerHttp: '',
  isTestnet: true,
}

export const PREVIEWNET_NETWORK: NetworkConfig = {
  id: 'previewnet',
  name: 'PreviewNet Testnet',
  parachainWs: 'wss://previewnet.substrate.dev/web3-storage',
  providerHttp: 'https://previewnet.substrate.dev/provider',
  relayChainWs: 'wss://previewnet-rpc.polkadot.io',
  isTestnet: true,
}

export const PASEO_NETWORK: NetworkConfig = {
  id: 'paseo',
  name: 'Paseo Testnet',
  parachainWs: '',
  providerHttp: '',
  isTestnet: true,
}

export const POLKADOT_NETWORK: NetworkConfig = {
  id: 'polkadot',
  name: 'Polkadot',
  parachainWs: '',
  providerHttp: '',
  isTestnet: false,
}

export const NETWORKS: Record<NetworkId, NetworkConfig> = {
  local: LOCAL_NETWORK,
  westend: WESTEND_NETWORK,
  previewnet: PREVIEWNET_NETWORK,
  paseo: PASEO_NETWORK,
  polkadot: POLKADOT_NETWORK,
  custom: {
    id: 'custom',
    name: 'Custom RPC',
    parachainWs: '',
    providerHttp: '',
    isTestnet: false,
  },
}

export const NETWORK_LIST: NetworkConfig[] = [
  LOCAL_NETWORK,
  WESTEND_NETWORK,
  PREVIEWNET_NETWORK,
  PASEO_NETWORK,
  POLKADOT_NETWORK,
]

export const DEFAULT_NETWORK_ID: NetworkId = 'previewnet'

export function createCustomNetwork(parachainWs: string, providerHttp: string): NetworkConfig {
  return {
    id: 'custom',
    name: 'Custom RPC',
    parachainWs,
    providerHttp,
    isTestnet: false,
  }
}
