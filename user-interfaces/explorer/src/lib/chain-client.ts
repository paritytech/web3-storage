// SPDX-License-Identifier: GPL-3.0-only

/**
 * Chain Client - read-only blockchain access via WebSocket.
 *
 * The generic connection lifecycle lives in the shared
 * @web3-storage/chain-client package (imported/re-exported below); only the
 * explorer-specific pieces (anchor clock, chain properties) stay here. This
 * app never submits an extrinsic.
 */

import { type PolkadotClient } from 'polkadot-api'
import { BehaviorSubject } from 'rxjs'
import { getSs58Prefix } from '@web3-storage/sdk'
import {
  clientReady$,
  connectToChain,
  disconnectFromChain as disconnectChain,
  getClient,
  requireApi,
  requireClient,
  subscribeToBlocks,
} from '@web3-storage/chain-client'

export type { PolkadotClient }
export { clientReady$, connectToChain, getClient, requireApi, requireClient, subscribeToBlocks }

/**
 * The pallet's anchor block — the clock every on-chain duration (agreement
 * expiry, challenge deadlines) is measured against. NOT the parachain height:
 * on live networks the two clocks differ by millions of blocks, so
 * pallet-clock comparisons must read this.
 */
export const anchorBlock$ = new BehaviorSubject<number | undefined>(undefined)

/**
 * Refresh [`anchorBlock$`] from the `current_anchor_block` runtime API. The
 * unsafe API resolves against live metadata, so no descriptor regeneration is
 * needed; on runtimes without the API the parachain height is the pallet
 * clock, so fall back to it.
 */
export async function refreshAnchorBlock(parachainBlock: number): Promise<void> {
  let anchor = parachainBlock
  const client = getClient()
  if (client) {
    try {
      anchor = Number(
        await client.getUnsafeApi().apis.StorageProviderApi.current_anchor_block()
      )
    } catch { /* pre-anchor runtime */ }
  }
  // Publish only if we are still on the connection this answer came from —
  // a call resolving after a disconnect/reconnect would resurrect a stale
  // anchor from the previous chain.
  if (getClient() !== client) return
  anchorBlock$.next(anchor)
}

export function disconnectFromChain(): void {
  disconnectChain()
  anchorBlock$.next(undefined)
}

// ─────────────────────────────────────────────────────────────────────────────
// Chain properties
// ─────────────────────────────────────────────────────────────────────────────

export async function getChainProperties(): Promise<{
  tokenDecimals: number
  tokenSymbol: string
  anchorBlockTimeMs: number
  ss58Prefix: number
  specName: string
  specVersion: number
  genesisHash: string
}> {
  // Defaults match the current runtime; overridden where the chain exposes
  // them via constants / spec data.
  let tokenDecimals = 12
  let tokenSymbol = 'UNIT'
  let anchorBlockTimeMs = 6000
  let ss58Prefix = getSs58Prefix()
  let specName = ''
  let specVersion = 0
  let genesisHash = ''

  const client = getClient()
  if (client) {
    const api = requireApi()
    try {
      const spec = await client.getChainSpecData()
      genesisHash = spec.genesisHash || genesisHash
      const props = spec.properties as { tokenDecimals?: number | number[]; tokenSymbol?: string | string[]; ss58Format?: number } | undefined
      if (props) {
        const dec = props.tokenDecimals
        if (typeof dec === 'number') tokenDecimals = dec
        else if (Array.isArray(dec) && dec.length > 0) tokenDecimals = dec[0]
        const sym = props.tokenSymbol
        if (typeof sym === 'string') tokenSymbol = sym
        else if (Array.isArray(sym) && sym.length > 0) tokenSymbol = sym[0]
        if (typeof props.ss58Format === 'number') ss58Prefix = props.ss58Format
      }
    } catch { /* use defaults */ }

    // If chain spec didn't have ss58Format, try the runtime constant
    try {
      ss58Prefix = await api.constants.System.SS58Prefix()
    } catch { /* use default */ }

    try {
      const version = await api.constants.System.Version()
      specName = version.spec_name
      specVersion = version.spec_version
    } catch { /* use default */ }

    // Anchor-clock tick — deliberately NOT Aura.SlotDuration: every duration
    // this UI formats is anchor-denominated (relay blocks), which the
    // parachain block time will stop matching when it changes. The unsafe API
    // resolves against live metadata, so no descriptor regeneration is needed;
    // runtimes without the API keep the 6s default.
    try {
      const millis = await client
        .getUnsafeApi()
        .apis.StorageProviderApi.anchor_block_time_millis()
      anchorBlockTimeMs = Number(millis)
    } catch { /* use default */ }
  }

  return { tokenDecimals, tokenSymbol, anchorBlockTimeMs, ss58Prefix, specName, specVersion, genesisHash }
}
