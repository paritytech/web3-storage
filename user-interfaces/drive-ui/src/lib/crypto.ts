// SPDX-License-Identifier: Apache-2.0

/**
 * PAPI-native crypto helpers. Derivation and SS58 encoding come from
 * @web3-storage/sdk (single source for the whole workspace); only the
 * address-validity check is app-local.
 */
import { getSs58AddressInfo } from "@polkadot-api/substrate-bindings";

export { seedToKeypair, toSs58, type Keypair } from "@web3-storage/sdk";

export function isValidSs58(address: string): boolean {
  return getSs58AddressInfo(address).isValid;
}
