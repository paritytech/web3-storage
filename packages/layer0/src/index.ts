// SPDX-License-Identifier: GPL-3.0-only

/**
 * @web3-storage/sdk — chain-interaction SDK for the storage parachain.
 *
 * Flat typed functions over the PAPI typed API. This package is the seed of
 * the dedicated TS SDK (issue #123): a future `Web3Storage.connect()` facade
 * wraps these functions with captured `{api, signer}` context — additive, no
 * rewrite. Test-only powers (sudo cleanup, expect-failure submission,
 * Playwright fixtures) live in @web3-storage/test-helpers, never here.
 *
 * pallet_revive helpers live in the `@web3-storage/sdk/revive` subpath so
 * consumers that never touch contracts don't pull viem into their graph.
 */

export * from "./address.js";
export * from "./client.js";
export * from "./signers.js";
export * from "./tx.js";
export * from "./waits.js";
export * from "./provider-http.js";
export * from "./pallets/storage-provider.js";
export * from "./pallets/drive-registry.js";
export * from "./pallets/s3-registry.js";
