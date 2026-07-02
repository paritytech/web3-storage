// SPDX-License-Identifier: Apache-2.0

export type NetworkId = 'local' | 'westend' | 'previewnet' | 'paseo' | 'polkadot' | 'custom'

export interface NetworkConfig {
  id: NetworkId
  name: string
  parachainWs: string
  providerHttp: string
  relayChainWs?: string
  isTestnet: boolean
}
