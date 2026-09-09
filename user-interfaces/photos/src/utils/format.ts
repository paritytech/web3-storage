// SPDX-License-Identifier: GPL-3.0-only

/**
 * Apply chain-derived configuration after the chain connects. For Photos the
 * only chain-derived setting we need is the SS58 prefix used to encode
 * addresses; the rest of the returned identity is published into chain state.
 */
export async function configureFromChain(props: {
  ss58Prefix: number
  specName: string
  specVersion: number
  genesisHash: string
}): Promise<{ name: string; version: string; genesisHash: string }> {
  // Dynamically import to avoid a circular dependency (wallet → chain-client → chain)
  const { updateSs58Prefix } = await import('@/state/wallet.state')
  await updateSs58Prefix(props.ss58Prefix)

  return {
    name: props.specName,
    version: String(props.specVersion),
    genesisHash: props.genesisHash,
  }
}

export function formatAddress(address: string, chars = 4): string {
  if (!address) return ''
  return `${address.slice(0, chars + 2)}...${address.slice(-chars)}`
}

/** Shorten a 0x-hex hash for display, keeping a head and tail. */
export function formatHash(hex: string, prefixChars = 6, suffixChars = 6): string {
  if (!hex || hex.length <= prefixChars + suffixChars + 2) return hex
  return `${hex.slice(0, prefixChars + 2)}...${hex.slice(-suffixChars)}`
}

/**
 * Render an atomic token amount (bigint, 12 decimals) as a human token string,
 * trimming trailing zeros and capping the fractional part for readability.
 */
export function formatTokens(atomic: bigint, decimals = 12, maxFractionDigits = 4): string {
  const base = 10n ** BigInt(decimals)
  const whole = atomic / base
  const frac = atomic % base
  if (frac === 0n) return whole.toString()
  // Left-pad the fractional part, then trim to maxFractionDigits and strip zeros.
  const fracStr = frac.toString().padStart(decimals, '0').slice(0, maxFractionDigits).replace(/0+$/, '')
  return fracStr ? `${whole}.${fracStr}` : whole.toString()
}

const BYTE_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'] as const

/** Render a byte count in binary units (1024-based), e.g. `1.5 MiB`. */
export function formatBytes(bytes: number | bigint): string {
  let value = Number(bytes)
  let unit = 0
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024
    unit++
  }
  const rounded = value >= 100 || Number.isInteger(value) ? Math.round(value) : Math.round(value * 10) / 10
  return `${rounded} ${BYTE_UNITS[unit]}`
}

export type ByteUnit = 'MiB' | 'GiB'

/** Convert a size entered in `MiB`/`GiB` to a byte count (bigint). */
export function bytesFromUnit(value: number, unit: ByteUnit): bigint {
  const factor = unit === 'GiB' ? 1024n ** 3n : 1024n ** 2n
  // Support fractional inputs (e.g. 1.5 GiB) without floating-point drift in
  // the final bigint: scale by 1000, multiply, then divide back.
  const scaled = BigInt(Math.round(value * 1000))
  return (scaled * factor) / 1000n
}
