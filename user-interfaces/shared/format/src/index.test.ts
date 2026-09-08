// SPDX-License-Identifier: GPL-3.0-only

import { afterEach, describe, it, expect } from 'vitest'
import {
  bytesFromUnit,
  configureFormat,
  formatAmount,
  formatBalance,
  formatBytes,
  formatBytesBinary,
  formatDuration,
  formatTokens,
  formatUnits,
  parseTokens,
  truncateHash,
} from './index'

// Tests that reconfigure the module must not leak into the rest of the suite.
afterEach(() => {
  configureFormat({ tokenDecimals: 12, tokenSymbol: 'UNIT', anchorBlockTimeMs: 6000 })
})

describe('formatBalance', () => {
  const UNIT = 1_000_000_000_000n // 12 decimals

  it('formats whole units with no decimals', () => {
    expect(formatBalance(5n * UNIT)).toBe('5')
    expect(formatBalance(1_000n * UNIT)).toBe('1,000')
  })

  it('formats values with significant fractional part', () => {
    expect(formatBalance(1_500_000_000_000n)).toBe('1.5')
    expect(formatBalance(1_250_000_000_000n)).toBe('1.25')
  })

  it('caps fractional part to 4 digits when whole > 0', () => {
    expect(formatBalance(1_123_456_789_012n)).toBe('1.1234')
  })

  it('formats very small values without truncating to empty', () => {
    expect(formatBalance(50n)).toBe('0.00000000005')
    expect(formatBalance(100n)).toBe('0.0000000001')
    expect(formatBalance(1_000_000n)).toBe('0.000001')
  })

  it('formats zero', () => {
    expect(formatBalance(0n)).toBe('0')
  })

  it('formats exactly 1 smallest unit', () => {
    expect(formatBalance(1n)).toBe('0.000000000001')
  })
})

describe('formatTokens', () => {
  const UNIT = 1_000_000_000_000n

  it('formats >= 1 UNIT with standard decimal', () => {
    expect(formatTokens(1n * UNIT)).toBe('1 UNIT')
    expect(formatTokens(1_000n * UNIT)).toBe('1,000 UNIT')
    expect(formatTokens(1_500_000_000_000n)).toBe('1.5 UNIT')
  })

  it('formats zero', () => {
    expect(formatTokens(0n)).toBe('0 UNIT')
  })

  // SI prefix: milli (10^9 – 10^12)
  it('uses milli prefix for values >= 10^9', () => {
    expect(formatTokens(1_000_000_000n)).toBe('1 milli UNIT')
    expect(formatTokens(500_000_000_000n)).toBe('500 milli UNIT')
    expect(formatTokens(1_500_000_000n)).toBe('1.5 milli UNIT')
  })

  // SI prefix: micro (10^6 – 10^9)
  it('uses micro prefix for values >= 10^6', () => {
    expect(formatTokens(1_000_000n)).toBe('1 micro UNIT')
    expect(formatTokens(500_000_000n)).toBe('500 micro UNIT')
    expect(formatTokens(1_234_567n)).toBe('1.2345 micro UNIT')
  })

  // SI prefix: nano (10^3 – 10^6)
  it('uses nano prefix for values >= 10^3', () => {
    expect(formatTokens(1_000n)).toBe('1 nano UNIT')
    expect(formatTokens(50_000n)).toBe('50 nano UNIT')
  })

  // SI prefix: pico (1 – 10^3)
  it('uses pico prefix for values < 10^3', () => {
    expect(formatTokens(1n)).toBe('1 pico UNIT')
    expect(formatTokens(50n)).toBe('50 pico UNIT')
    expect(formatTokens(100n)).toBe('100 pico UNIT')
    expect(formatTokens(999n)).toBe('999 pico UNIT')
  })

  // Regression: the per-app copies gated milli on >= 9 decimals and nano on
  // >= 3, so a 3–8 decimal chain computed 10n ** negative and threw a
  // RangeError. Every chain today uses 12 decimals, which masked it.
  it('does not crash and picks sensible prefixes on low-decimal chains', () => {
    configureFormat({ tokenDecimals: 6, tokenSymbol: 'USDX' })
    expect(formatTokens(1_000_000n)).toBe('1 USDX')
    expect(formatTokens(500_000n)).toBe('500 milli USDX')
    expect(formatTokens(5n)).toBe('5 micro USDX')

    configureFormat({ tokenDecimals: 3, tokenSymbol: 'MILX' })
    expect(formatTokens(1_000n)).toBe('1 MILX')
    expect(formatTokens(5n)).toBe('5 milli MILX')

    configureFormat({ tokenDecimals: 0, tokenSymbol: 'INTX' })
    expect(formatTokens(5n)).toBe('5 INTX')
  })

  it('formats 9-decimal chains across all prefixes', () => {
    configureFormat({ tokenDecimals: 9, tokenSymbol: 'NINE' })
    expect(formatTokens(1_000_000_000n)).toBe('1 NINE')
    expect(formatTokens(1_000_000n)).toBe('1 milli NINE')
    expect(formatTokens(1_000n)).toBe('1 micro NINE')
    expect(formatTokens(1n)).toBe('1 nano NINE')
  })

  // Regression: the terminal 'pico' entry was a hardcoded literal, only
  // correct at exactly 12 decimals — a 10-decimal chain rendered 5×10^-10
  // as "5 pico" (off by 100×). Pico is now derived like the other prefixes,
  // and amounts below every applicable prefix render as exact decimals.
  it('never mislabels sub-prefix amounts on non-multiple-of-3 decimals', () => {
    configureFormat({ tokenDecimals: 10, tokenSymbol: 'TEN' })
    expect(formatTokens(10n)).toBe('1 nano TEN')
    expect(formatTokens(5n)).toBe('0.0000000005 TEN')

    configureFormat({ tokenDecimals: 15, tokenSymbol: 'BIG' })
    expect(formatTokens(1_000n)).toBe('1 pico BIG')
    expect(formatTokens(5n)).toBe('0.000000000000005 BIG')
  })
})

describe('parseTokens', () => {
  it('round-trips whole and fractional user input at config decimals', () => {
    expect(parseTokens('1000')).toBe(1_000_000_000_000_000n)
    expect(parseTokens('1.5')).toBe(1_500_000_000_000n)
    expect(parseTokens('0.000000000001')).toBe(1n)
    expect(parseTokens('')).toBe(0n)
  })

  it('truncates fractional digits beyond the config decimals', () => {
    expect(parseTokens('0.0000000000019')).toBe(1n)
  })
})

describe('formatDuration', () => {
  it('humanizes anchor blocks using the configured block time', () => {
    expect(formatDuration(0)).toBe('0 blocks')
    expect(formatDuration(10)).toBe('1m')
    expect(formatDuration(600)).toBe('1h 0m')
    expect(formatDuration(4_000_000_000)).toBe('no limit')
    configureFormat({ anchorBlockTimeMs: 12_000 })
    expect(formatDuration(10)).toBe('2m')
  })
})

describe('formatUnits', () => {
  it('renders a grouped whole with a fixed 2-digit fraction', () => {
    expect(formatUnits(1_000_500_000_000_000n)).toBe('1,000.50')
    expect(formatUnits(1_000_000_000_000n)).toBe('1.00')
    expect(formatUnits(0n)).toBe('0.00')
  })

  it('accepts explicit decimals', () => {
    expect(formatUnits(1_500_000n, 6)).toBe('1.50')
  })
})

describe('formatAmount', () => {
  it('trims trailing zeros and caps fraction digits', () => {
    expect(formatAmount(1_500_000_000_000n)).toBe('1.5')
    expect(formatAmount(1_000_000_000_000n)).toBe('1')
    expect(formatAmount(1_123_456_000_000n)).toBe('1.1234')
    expect(formatAmount(0n)).toBe('0')
  })
})

// Byte units use binary (base 1024) — colloquial "GB" == 2^30 bytes,
// matching what most users mean when they type "1 GB".
describe('formatBytes', () => {
  it('formats zero and sub-KB values', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(1)).toBe('1 B')
    expect(formatBytes(500)).toBe('500 B')
    expect(formatBytes(1023)).toBe('1023 B')
  })

  it('uses base 1024 for unit transitions', () => {
    expect(formatBytes(1024)).toBe('1 KB')
    expect(formatBytes(1024 ** 2)).toBe('1 MB')
    expect(formatBytes(1024 ** 3)).toBe('1 GB')
    expect(formatBytes(1024 ** 4)).toBe('1 TB')
    expect(formatBytes(1024 ** 5)).toBe('1 PB')
    expect(formatBytes(1024 ** 6)).toBe('1 EB')
  })

  it('rejects SI gigabyte (10^9) as still being MB', () => {
    // 10^9 bytes is only ~954 MiB — must NOT be labelled "1 GB".
    expect(formatBytes(1_000_000_000)).toBe('953.67 MB')
  })

  it('strips trailing zeros and rounds to two decimals', () => {
    expect(formatBytes(1500)).toBe('1.46 KB')
    expect(formatBytes(1024 * 1024 * 1.5)).toBe('1.5 MB')
  })

  it('accepts bigint input', () => {
    expect(formatBytes(1024n ** 3n)).toBe('1 GB')
    expect(formatBytes(1_073_741_824n)).toBe('1 GB')
  })

  it('clamps very large values to EB', () => {
    expect(formatBytes(1024 ** 7)).toBe('1024 EB')
  })
})

describe('formatBytesBinary', () => {
  it('uses explicit binary units with one-decimal rounding', () => {
    expect(formatBytesBinary(0)).toBe('0 B')
    expect(formatBytesBinary(1024)).toBe('1 KiB')
    expect(formatBytesBinary(1024 * 1024 * 1.5)).toBe('1.5 MiB')
    expect(formatBytesBinary(150 * 1024)).toBe('150 KiB')
  })
})

describe('bytesFromUnit', () => {
  it('converts MiB/GiB including fractional inputs without drift', () => {
    expect(bytesFromUnit(1, 'MiB')).toBe(1_048_576n)
    expect(bytesFromUnit(1.5, 'GiB')).toBe((3n * 1024n ** 3n) / 2n)
  })
})

describe('truncateHash', () => {
  it('keeps head and tail, passing short values through', () => {
    expect(truncateHash('abcdefghijkl')).toBe('abcdef...ijkl')
    expect(truncateHash('short')).toBe('short')
  })
})
