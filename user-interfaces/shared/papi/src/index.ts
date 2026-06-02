/**
 * Shared PAPI utilities used across all UIs.
 */

import { ss58Decode } from '@polkadot-labs/hdkd-helpers'

/** Compare two SS58 addresses by raw public key bytes (prefix-agnostic). */
export function isSameAddress(a: string, b: string): boolean {
  try {
    const [aBytes] = ss58Decode(a)
    const [bBytes] = ss58Decode(b)
    if (aBytes.length !== bBytes.length) return false
    for (let i = 0; i < aBytes.length; i++) {
      if (aBytes[i] !== bBytes[i]) return false
    }
    return true
  } catch {
    return false
  }
}
