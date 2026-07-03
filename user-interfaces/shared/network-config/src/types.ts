// SPDX-License-Identifier: GPL-3.0-only

export type NetworkId = 'local' | 'westend' | 'previewnet' | 'paseo' | 'polkadot' | 'custom'

export interface NetworkConfig {
  id: NetworkId
  name: string
  parachainWs: string
  providerHttp: string
  relayChainWs?: string
  isTestnet: boolean
  /**
   * Deployed Photos dApp contract address (H160, `0x`-prefixed). Deployed once
   * per network; left unset until a deployment exists. The Photos UI also
   * accepts a `VITE_PHOTOS_CONTRACT` / `localStorage['photos.contract']` dev
   * override (see `photos/src/lib/photos-contract.ts`).
   */
  photosContract?: string
}
