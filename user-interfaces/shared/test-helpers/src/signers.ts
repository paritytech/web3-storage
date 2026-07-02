// SPDX-License-Identifier: Apache-2.0

/**
 * Dev signers come from @web3-storage/sdk (single derivation + SS58 source
 * for the whole workspace); re-exported here so existing test imports keep
 * working unchanged.
 */
export {
  Alice,
  Bob,
  Charlie,
  Dave,
  Eve,
  Ferdie,
  devSigners,
  getDevSigner,
  type DevAccountName,
  type DevSigner,
} from "@web3-storage/sdk";
