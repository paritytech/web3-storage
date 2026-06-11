/**
 * Address and byte-level helpers. SS58 encoding state (prefix) lives in
 * @web3-storage/papi — the single owner of the descriptor set — so every
 * consumer renders addresses with the same, runtime-derived prefix.
 */

export {
  getSs58Prefix,
  isSameAddress,
  parseMultiaddrToUrl,
  resolveProviderEndpoint,
  setSs58Prefix,
  toSs58,
  type ParachainApi,
} from "@web3-storage/papi";

import { isSameAddress } from "@web3-storage/papi";

/**
 * Compare two SS58 addresses by their underlying public key, regardless of
 * the prefix each one was encoded with. Alias kept for the PAPI examples,
 * which predate `isSameAddress`.
 */
export const sameAddress = isSameAddress;

export { asHex, bytesEq, hexToBytes, toHex } from "@web3-storage/core";
