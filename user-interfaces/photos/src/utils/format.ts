// SPDX-License-Identifier: GPL-3.0-only

import { configureFormat } from '@web3-storage/format'
import { updateSs58Prefix } from '@/state/wallet.state'

/**
 * Apply chain-derived configuration after the chain connects: the SS58 prefix
 * used to encode addresses, and the token decimals and symbol from the chain
 * spec properties used to format balances. Display formatters live in the
 * shared `@web3-storage/format` package.
 */
export function configureFromChain(props: { ss58Prefix: number; specProperties?: unknown }): void {
  updateSs58Prefix(props.ss58Prefix)
  const spec = props.specProperties as { tokenDecimals?: number | number[]; tokenSymbol?: string | string[] } | undefined
  configureFormat({
    tokenDecimals: Array.isArray(spec?.tokenDecimals) ? spec.tokenDecimals[0] : spec?.tokenDecimals,
    tokenSymbol: Array.isArray(spec?.tokenSymbol) ? spec.tokenSymbol[0] : spec?.tokenSymbol,
  })
}
