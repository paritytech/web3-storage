// SPDX-License-Identifier: GPL-3.0-only

export type NetworkId = 'local' | 'westend' | 'previewnet' | 'paseo' | 'polkadot' | 'custom'

export interface NetworkConfig {
  id: NetworkId
  name: string
  parachainWs: string
  providerHttp: string
  relayChainWs?: string
  isTestnet: boolean
}
