export type NetworkId = 'local' | 'westend' | 'paseo' | 'polkadot' | 'custom'

export interface NetworkConfig {
  id: NetworkId
  name: string
  parachainWs: string
  providerHttp: string
  relayChainWs?: string
  explorerUrl?: string
  isTestnet: boolean
}
