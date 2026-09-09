// SPDX-License-Identifier: GPL-3.0-only

/**
 * Photos' chain interaction is just the generic connection lifecycle (plus an
 * unsigned `ReviveApi.call` read in `lib/photos-contract.ts`), so re-export the
 * shared chain-client. Thin module kept so `@/lib/chain-client` stays stable.
 */

export * from '@web3-storage/chain-client'
