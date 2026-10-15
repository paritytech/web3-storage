// SPDX-License-Identifier: GPL-3.0-only

import { getSs58AddressInfo } from "@polkadot-api/substrate-bindings";

export function isValidSs58(address: string): boolean {
  return getSs58AddressInfo(address).isValid;
}
