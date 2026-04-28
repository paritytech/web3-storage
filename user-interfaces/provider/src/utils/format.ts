// Chain-derived configuration — initialized with defaults, updated from chain
// metadata at connection time via configureFromChain().
let tokenDecimals = 12
let tokenSymbol = 'UNIT'
let blockTimeMs = 6000
let UNIT = 10n ** BigInt(tokenDecimals)

/**
 * Update formatting configuration from chain metadata.
 * Called once after chain connection to replace hardcoded defaults.
 */
export function configureFromChain(props: {
  tokenDecimals: number
  tokenSymbol: string
  blockTimeMs: number
}) {
  tokenDecimals = props.tokenDecimals
  tokenSymbol = props.tokenSymbol
  blockTimeMs = props.blockTimeMs
  UNIT = 10n ** BigInt(tokenDecimals)
}

export function getTokenSymbol(): string {
  return tokenSymbol
}

export function getTokenDecimals(): number {
  return tokenDecimals
}

export function formatAddress(address: string, chars = 4): string {
  if (!address) return ''
  return `${address.slice(0, chars + 2)}...${address.slice(-chars)}`
}

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

export function formatTokens(balance: bigint): string {
  if (balance === 0n) return `0 ${tokenSymbol}`

  if (balance >= UNIT) {
    return `${formatBalance(balance)} ${tokenSymbol}`
  }

  // SI prefixes for sub-unit amounts (derived from token decimals)
  const siPrefixes: { threshold: bigint; divisor: bigint; label: string }[] = []
  if (tokenDecimals >= 9) siPrefixes.push({ threshold: 10n ** BigInt(tokenDecimals - 3), divisor: 10n ** BigInt(tokenDecimals - 3), label: 'milli' })
  if (tokenDecimals >= 6) siPrefixes.push({ threshold: 10n ** BigInt(tokenDecimals - 6), divisor: 10n ** BigInt(tokenDecimals - 6), label: 'micro' })
  if (tokenDecimals >= 3) siPrefixes.push({ threshold: 10n ** BigInt(tokenDecimals - 9), divisor: 10n ** BigInt(Math.max(0, tokenDecimals - 9)), label: 'nano' })
  siPrefixes.push({ threshold: 1n, divisor: 1n, label: 'pico' })

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

  return `${formatBalance(balance)} ${tokenSymbol}`
}

export function parseTokens(value: string): bigint {
  const [whole, fraction = ''] = value.split('.')
  const paddedFraction = fraction.padEnd(tokenDecimals, '0').slice(0, tokenDecimals)
  return BigInt(whole || '0') * UNIT + BigInt(paddedFraction)
}

export function formatBytes(bytes: number | bigint): string {
  const b = typeof bytes === 'bigint' ? Number(bytes) : bytes
  const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB']
  let unitIndex = 0
  let value = b

  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024
    unitIndex++
  }

  return `${value.toFixed(unitIndex === 0 ? 0 : 2)} ${units[unitIndex]}`
}

export function formatDuration(blocks: number): string {
  if (blocks === 0) return '0 blocks'
  if (blocks >= 4_000_000_000) return 'no limit'

  const seconds = blocks * (blockTimeMs / 1000)
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

export function formatBlockNumber(block: number | bigint): string {
  return `#${Number(block).toLocaleString()}`
}

export function formatPercentage(value: number, decimals = 2): string {
  return `${(value * 100).toFixed(decimals)}%`
}

export function formatHash(hex: string, prefixChars = 6, suffixChars = 6): string {
  if (!hex || hex.length <= prefixChars + suffixChars + 2) return hex
  return `${hex.slice(0, prefixChars + 2)}...${hex.slice(-suffixChars)}`
}

export function formatDate(timestamp: number): string {
  return new Date(timestamp).toLocaleDateString('en-US', {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

export function formatRelativeTime(timestamp: number): string {
  const now = Date.now()
  const diff = now - timestamp

  const seconds = Math.floor(diff / 1000)
  const minutes = Math.floor(seconds / 60)
  const hours = Math.floor(minutes / 60)
  const days = Math.floor(hours / 24)

  if (days > 0) return `${days}d ago`
  if (hours > 0) return `${hours}h ago`
  if (minutes > 0) return `${minutes}m ago`
  return 'just now'
}
