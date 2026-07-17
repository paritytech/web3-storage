// SPDX-License-Identifier: Apache-2.0

// Address-format primitives. Pure byte/SS58 computation with no chain coupling,
// so they belong in core rather than the layer-0 chain binding.

import { ss58Address } from "@polkadot-labs/hdkd-helpers";

/** A substrate account derived from an H160: its public key + SS58 address. */
export interface MappedAccount {
  publicKey: Uint8Array;
  address: string;
}

/**
 * Substrate account `AccountId32Mapper` assigns to an unmapped H160 (e.g. a
 * deployed contract): the 20 address bytes followed by 12 bytes of `0xEE`. Use
 * it as the `owner` of negotiated terms when a contract forwards them on a
 * user's behalf. The inverse is `substrateToH160` in
 * `@web3-storage/layer0/revive` (it needs viem).
 */
export function h160ToSubstrate(addressBytes: Uint8Array): MappedAccount {
  const publicKey = new Uint8Array(32).fill(0xee);
  publicKey.set(addressBytes, 0);
  return { publicKey, address: ss58Address(publicKey) };
}
