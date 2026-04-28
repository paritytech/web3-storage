import { describe, it, expect } from 'vitest'
import { formatBalance, formatTokens } from './format'

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
})
