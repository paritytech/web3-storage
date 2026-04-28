const UNIT = 1_000_000_000_000n // 12 decimals

export function formatAddress(address: string, chars = 4): string {
  if (!address) return ''
  return `${address.slice(0, chars + 2)}...${address.slice(-chars)}`
}

export function formatBalance(balance: bigint, decimals = 12): string {
  const divisor = 10n ** BigInt(decimals)
  const whole = balance / divisor
  const fraction = balance % divisor

  if (fraction === 0n) {
    return whole.toLocaleString()
  }

  const fullFraction = fraction.toString().padStart(decimals, '0')
  const maxDecimals = whole > 0n ? 4 : decimals
  const display = fullFraction.slice(0, maxDecimals).replace(/0+$/, '')

  if (!display) {
    return whole.toLocaleString()
  }

  return `${whole.toLocaleString()}.${display}`
}

// SI prefixes for sub-unit amounts (12-decimal token)
const SI_PREFIXES: { threshold: bigint; divisor: bigint; label: string }[] = [
  { threshold: 1_000_000_000n, divisor: 1_000_000_000n, label: 'milli' },  // 10^9
  { threshold: 1_000_000n,     divisor: 1_000_000n,     label: 'micro' },  // 10^6
  { threshold: 1_000n,         divisor: 1_000n,         label: 'nano' },   // 10^3
  { threshold: 1n,             divisor: 1n,             label: 'pico' },   // 10^0
]

export function formatTokens(balance: bigint): string {
  if (balance === 0n) return '0 UNIT'

  // >= 1 UNIT: use standard decimal format
  if (balance >= UNIT) {
    return `${formatBalance(balance)} UNIT`
  }

  // < 1 UNIT: pick the best SI prefix
  for (const { threshold, divisor, label } of SI_PREFIXES) {
    if (balance >= threshold) {
      const whole = balance / divisor
      const fraction = balance % divisor
      if (fraction === 0n) {
        return `${whole.toLocaleString()} ${label} UNIT`
      }
      // Show up to 4 significant fractional digits
      const fracDigits = Math.log10(Number(divisor)) || 1
      const fracStr = fraction.toString().padStart(fracDigits, '0').replace(/0+$/, '').slice(0, 4)
      return `${whole.toLocaleString()}.${fracStr} ${label} UNIT`
    }
  }

  return `${formatBalance(balance)} UNIT`
}

export function parseTokens(value: string): bigint {
  const [whole, fraction = ''] = value.split('.')
  const paddedFraction = fraction.padEnd(12, '0').slice(0, 12)
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
  // u32::MAX or similarly huge values = "no limit"
  if (blocks >= 4_000_000_000) return 'no limit'

  // Assuming 6 second blocks
  const seconds = blocks * 6
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
  return `${seconds}s`
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
