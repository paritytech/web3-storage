// SPDX-License-Identifier: GPL-3.0-only

/**
 * Shared display formatters for the UI apps — the single home for the token,
 * byte, address, and time formatting that used to be copied per app.
 *
 * Three token-rendering styles coexist because the apps render amounts in
 * genuinely different contexts; each style exists exactly once here:
 * - `formatTokens` — chain-configured decimals/symbol with SI prefixes for
 *   sub-unit amounts ("1,000 UNIT", "500 milli UNIT"). Used by dashboards.
 * - `formatUnits` — grouped whole part with a fixed 2-digit fraction and no
 *   symbol ("1,000.50"). Used for header balances and price lists.
 * - `formatAmount` — ungrouped, trailing-zero-trimmed fraction ("1.5").
 *   Used where the symbol/grouping is provided by surrounding copy.
 *
 * Byte rendering likewise: `formatBytes` (colloquial "KB/MB", base 1024) and
 * `formatBytesBinary` ("KiB/MiB").
 */

// Chain-derived configuration — initialized with defaults, updated from chain
// metadata at connection time via configureFormat().
let tokenDecimals = 12
let tokenSymbol = 'UNIT'
// Milliseconds per anchor block (the pallet clock all on-chain durations
// use), not the parachain block time.
let anchorBlockTimeMs = 6000
let UNIT = 10n ** BigInt(tokenDecimals)

/**
 * Apply chain-derived formatting configuration. Called after the chain
 * connects by the apps that render chain-configured amounts (provider,
 * explorer); apps that never call it format against the 12-decimal defaults
 * below. Anything app-specific that used to ride along here (SS58 prefix
 * updates, chain identity) is the caller's business.
 */
export function configureFormat(props: {
  tokenDecimals?: number
  tokenSymbol?: string
  anchorBlockTimeMs?: number
}): void {
  if (props.tokenDecimals !== undefined) {
    tokenDecimals = props.tokenDecimals
    UNIT = 10n ** BigInt(tokenDecimals)
  }
  if (props.tokenSymbol !== undefined) tokenSymbol = props.tokenSymbol
  if (props.anchorBlockTimeMs !== undefined) anchorBlockTimeMs = props.anchorBlockTimeMs
}

export function getTokenSymbol(): string {
  return tokenSymbol
}

export function getTokenDecimals(): number {
  return tokenDecimals
}

// ─────────────────────────────────────────────────────────────────────────────
// Token amounts
// ─────────────────────────────────────────────────────────────────────────────

export function formatBalance(balance: bigint, decimals?: number): string {
  const dec = decimals ?? tokenDecimals
  const divisor = 10n ** BigInt(dec)
  const whole = balance / divisor
  const fraction = balance % divisor

  if (fraction === 0n) {
    return whole.toLocaleString()
  }

  const fullFraction = fraction.toString().padStart(dec, '0')
  const maxDecimals = whole > 0n ? 4 : dec
  const display = fullFraction.slice(0, maxDecimals).replace(/0+$/, '')

  if (!display) {
    return whole.toLocaleString()
  }

  return `${whole.toLocaleString()}.${display}`
}

/**
 * Chain-configured token string with SI prefixes for sub-unit amounts.
 * Each prefix needs `tokenDecimals` large enough for a non-negative
 * exponent: milli = 10^(dec-3), micro = 10^(dec-6), nano = 10^(dec-9) — a
 * negative bigint exponent throws, so the gates must match the exponents.
 */
export function formatTokens(balance: bigint): string {
  if (balance === 0n) return `0 ${tokenSymbol}`

  if (balance >= UNIT) {
    return `${formatBalance(balance)} ${tokenSymbol}`
  }

  const siPrefixes: { threshold: bigint; divisor: bigint; label: string }[] = []
  if (tokenDecimals >= 3) siPrefixes.push({ threshold: 10n ** BigInt(tokenDecimals - 3), divisor: 10n ** BigInt(tokenDecimals - 3), label: 'milli' })
  if (tokenDecimals >= 6) siPrefixes.push({ threshold: 10n ** BigInt(tokenDecimals - 6), divisor: 10n ** BigInt(tokenDecimals - 6), label: 'micro' })
  if (tokenDecimals >= 9) siPrefixes.push({ threshold: 10n ** BigInt(tokenDecimals - 9), divisor: 10n ** BigInt(tokenDecimals - 9), label: 'nano' })
  if (tokenDecimals >= 12) siPrefixes.push({ threshold: 10n ** BigInt(tokenDecimals - 12), divisor: 10n ** BigInt(tokenDecimals - 12), label: 'pico' })

  for (const { threshold, divisor, label } of siPrefixes) {
    if (balance >= threshold) {
      const whole = balance / divisor
      const fraction = balance % divisor
      if (fraction === 0n) {
        return `${whole.toLocaleString()} ${label} ${tokenSymbol}`
      }
      const fracDigits = Math.log10(Number(divisor)) || 1
      const fracStr = fraction.toString().padStart(fracDigits, '0').replace(/0+$/, '').slice(0, 4)
      return `${whole.toLocaleString()}.${fracStr} ${label} ${tokenSymbol}`
    }
  }

  // Below every applicable prefix (only possible when tokenDecimals is not a
  // multiple of 3): render the exact decimal rather than mislabel the unit.
  return `${formatBalance(balance)} ${tokenSymbol}`
}

/**
 * Grouped whole part with a fixed 2-digit fraction, no symbol ("1,000.50").
 */
export function formatUnits(units: bigint, decimals: number = tokenDecimals): string {
  const divisor = 10n ** BigInt(decimals)
  const whole = units / divisor
  const frac = units % divisor
  const fracStr = frac.toString().padStart(decimals, '0').slice(0, 2)
  return `${whole.toLocaleString()}.${fracStr}`
}

/**
 * Ungrouped token amount with the fractional part trimmed of trailing zeros
 * and capped for readability ("1.5").
 */
export function formatAmount(
  atomic: bigint,
  decimals: number = tokenDecimals,
  maxFractionDigits = 4
): string {
  const base = 10n ** BigInt(decimals)
  const whole = atomic / base
  const frac = atomic % base
  if (frac === 0n) return whole.toString()
  const fracStr = frac.toString().padStart(decimals, '0').slice(0, maxFractionDigits).replace(/0+$/, '')
  return fracStr ? `${whole}.${fracStr}` : whole.toString()
}

export function parseTokens(value: string): bigint {
  const [whole, fraction = ''] = value.split('.')
  const paddedFraction = fraction.padEnd(tokenDecimals, '0').slice(0, tokenDecimals)
  return BigInt(whole || '0') * UNIT + BigInt(paddedFraction)
}

// ─────────────────────────────────────────────────────────────────────────────
// Bytes
// ─────────────────────────────────────────────────────────────────────────────

// Byte units use binary (base 1024) — colloquial "GB" == 2^30 bytes,
// matching what most users mean when they type "1 GB".
export function formatBytes(bytes: number | bigint): string {
  const b = typeof bytes === 'bigint' ? Number(bytes) : bytes
  if (b === 0) return '0 B'

  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB', 'PB', 'EB']
  const i = Math.min(Math.floor(Math.log(b) / Math.log(k)), sizes.length - 1)

  return `${parseFloat((b / Math.pow(k, i)).toFixed(2))} ${sizes[i]}`
}

const BINARY_BYTE_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'] as const

/** Render a byte count in explicit binary units, e.g. `1.5 MiB`. */
export function formatBytesBinary(bytes: number | bigint): string {
  let value = Number(bytes)
  let unit = 0
  while (value >= 1024 && unit < BINARY_BYTE_UNITS.length - 1) {
    value /= 1024
    unit++
  }
  const rounded = value >= 100 || Number.isInteger(value) ? Math.round(value) : Math.round(value * 10) / 10
  return `${rounded} ${BINARY_BYTE_UNITS[unit]}`
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

// ─────────────────────────────────────────────────────────────────────────────
// Addresses, hashes, blocks, time
// ─────────────────────────────────────────────────────────────────────────────

export function formatAddress(address: string, chars = 4): string {
  if (!address) return ''
  return `${address.slice(0, chars + 2)}...${address.slice(-chars)}`
}

/** Shorten a 0x-hex hash for display, keeping a head and tail. */
export function formatHash(hex: string, prefixChars = 6, suffixChars = 6): string {
  if (!hex || hex.length <= prefixChars + suffixChars + 2) return hex
  return `${hex.slice(0, prefixChars + 2)}...${hex.slice(-suffixChars)}`
}

/** Shorten an arbitrary hash/id, head-and-tail, without assuming an 0x prefix. */
export function truncateHash(hash: string, startChars = 6, endChars = 4): string {
  if (hash.length <= startChars + endChars) return hash
  return `${hash.slice(0, startChars)}...${hash.slice(-endChars)}`
}

export function formatBlockNumber(block: number | bigint): string {
  return `#${Number(block).toLocaleString()}`
}

/** Humanize a duration given in anchor blocks (the pallet clock). */
export function formatDuration(blocks: number): string {
  if (blocks === 0) return '0 blocks'
  if (blocks >= 4_000_000_000) return 'no limit'

  const seconds = blocks * (anchorBlockTimeMs / 1000)
  const minutes = Math.floor(seconds / 60)
  const hours = Math.floor(minutes / 60)
  const days = Math.floor(hours / 24)

  if (days > 365) {
    return `${blocks.toLocaleString()} blocks`
  }
  if (days > 0) {
    return `${days}d ${hours % 24}h`
  }
  if (hours > 0) {
    return `${hours}h ${minutes % 60}m`
  }
  if (minutes > 0) {
    return `${minutes}m`
  }
  return `${Math.round(seconds)}s`
}

export function formatTimestamp(timestamp: number): string {
  return new Date(timestamp).toLocaleString()
}
