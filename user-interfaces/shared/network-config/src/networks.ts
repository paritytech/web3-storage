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
  parachainWs: 'wss://westend-storage-rpc.polkadot.io',
  providerHttp: 'https://westend-storage-provider.polkadot.io',
  relayChainWs: 'wss://westend-rpc.polkadot.io',
  explorerUrl: 'https://westend.subscan.io',
  isTestnet: true,
}

export const PASEO_NETWORK: NetworkConfig = {
  id: 'paseo',
  name: 'Paseo Testnet',
  parachainWs: 'wss://paseo-storage-rpc.polkadot.io',
  providerHttp: 'https://paseo-storage-provider.polkadot.io',
  relayChainWs: 'wss://paseo-rpc.polkadot.io',
  explorerUrl: 'https://paseo.subscan.io',
  isTestnet: true,
}

export const POLKADOT_NETWORK: NetworkConfig = {
  id: 'polkadot',
  name: 'Polkadot',
  parachainWs: 'wss://storage-rpc.polkadot.io',
  providerHttp: 'https://storage-provider.polkadot.io',
  relayChainWs: 'wss://rpc.polkadot.io',
  explorerUrl: 'https://polkadot.subscan.io',
  isTestnet: false,
}

export const NETWORKS: Record<NetworkId, NetworkConfig> = {
  local: LOCAL_NETWORK,
  westend: WESTEND_NETWORK,
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
  PASEO_NETWORK,
  POLKADOT_NETWORK,
]

export const DEFAULT_NETWORK_ID: NetworkId = 'local'

export function createCustomNetwork(parachainWs: string, providerHttp: string): NetworkConfig {
  return {
    id: 'custom',
    name: 'Custom RPC',
    parachainWs,
    providerHttp,
    isTestnet: false,
  }
}
