// SPDX-License-Identifier: GPL-3.0-only

import { describe, it, expect } from 'vitest'
import {
  agreementStatus,
  bucketQuotas,
  reputationScore,
  summarize,
  type AgreementRow,
  type NetworkSnapshot,
  type ProviderRow,
} from './explorer-client'

function agreement(overrides: Partial<AgreementRow>): AgreementRow {
  return {
    bucketId: 1,
    provider: '5Provider',
    owner: '5Owner',
    maxBytes: 100n,
    paymentLocked: 0n,
    pricePerByte: 1n,
    expiresAt: 1000,
    startedAt: 1,
    role: 'Primary',
    extensionsBlocked: false,
    ...overrides,
  }
}

function provider(overrides: Partial<ProviderRow>): ProviderRow {
  return {
    address: '5Provider',
    multiaddr: '/ip4/127.0.0.1/tcp/3333',
    stake: 0n,
    committedBytes: 0n,
    settings: {
      minDuration: 0,
      maxDuration: 0,
      pricePerByte: 0n,
      acceptingPrimary: true,
      replicaSyncPrice: undefined,
      acceptingExtensions: true,
      maxCapacity: 0n,
    },
    stats: {
      registeredAt: 0,
      agreementsTotal: 0,
      agreementsExtended: 0,
      agreementsNotExtended: 0,
      agreementsBurned: 0,
      totalBytesCommitted: 0n,
      challengesDefendedAuthorized: 0,
      challengesDefendedPublic: 0,
      challengesFailed: 0,
    },
    deregisterAt: undefined,
    ...overrides,
  }
}

describe('agreementStatus', () => {
  it('ignores extensions_blocked — a no-renewals agreement is live until expiry', () => {
    expect(agreementStatus(agreement({ extensionsBlocked: true, expiresAt: 1000 }), 100)).toBe(
      'active'
    )
    expect(agreementStatus(agreement({ extensionsBlocked: true, expiresAt: 10 }), 100)).toBe(
      'expired'
    )
  })

  it('is expired only strictly after the anchor passes expires_at', () => {
    expect(agreementStatus(agreement({ expiresAt: 100 }), 101)).toBe('expired')
    expect(agreementStatus(agreement({ expiresAt: 100 }), 100)).toBe('active')
  })

  it('never expires while the anchor clock is unseeded (0)', () => {
    expect(agreementStatus(agreement({ expiresAt: 100 }), 0)).toBe('active')
  })

  it('treats expires_at 0 as never expiring', () => {
    expect(agreementStatus(agreement({ expiresAt: 0 }), 999)).toBe('active')
  })
})

describe('bucketQuotas', () => {
  it('sums max_bytes per bucket, including no-renewals agreements', () => {
    const quotas = bucketQuotas([
      agreement({ bucketId: 1, maxBytes: 100n }),
      agreement({ bucketId: 1, maxBytes: 50n, provider: '5Other' }),
      agreement({ bucketId: 2, maxBytes: 7n }),
      agreement({ bucketId: 1, maxBytes: 999n, extensionsBlocked: true }),
    ])
    expect(quotas.get(1)).toBe(1149n)
    expect(quotas.get(2)).toBe(7n)
  })
})

// Mirrors the pallet's reputation_score (runtime_api.rs) — keep in lockstep.
describe('reputationScore', () => {
  const stats = (authorized: number, pub_: number, failed: number) => ({
    ...provider({}).stats,
    challengesDefendedAuthorized: authorized,
    challengesDefendedPublic: pub_,
    challengesFailed: failed,
  })

  it('scores 100 with no resolved challenges (benefit of the doubt)', () => {
    expect(reputationScore(stats(0, 0, 0))).toBe(100)
  })

  it('is the floored share of resolved challenges defended, both tiers counted', () => {
    expect(reputationScore(stats(2, 1, 1))).toBe(75)
    expect(reputationScore(stats(1, 0, 2))).toBe(33)
    expect(reputationScore(stats(0, 0, 5))).toBe(0)
    expect(reputationScore(stats(4, 3, 0))).toBe(100)
  })
})

describe('summarize', () => {
  it('computes bigint-safe totals and anchor-filtered active count', () => {
    const snapshot: NetworkSnapshot = {
      providers: [
        provider({ stake: 10n ** 15n, committedBytes: 5n }),
        provider({ address: '5Other', stake: 10n ** 15n, committedBytes: 7n }),
      ],
      agreements: [
        agreement({ expiresAt: 100 }),
        agreement({ bucketId: 2, expiresAt: 10 }),
        agreement({ bucketId: 3, expiresAt: 100, extensionsBlocked: true }),
      ],
      buckets: [],
      openChallenges: [],
      bucketsEverCreated: 3,
      failedSections: [],
      fetchedAt: 0,
    }
    const stats = summarize(snapshot, 50)
    expect(stats.providerCount).toBe(2)
    expect(stats.totalStake).toBe(2n * 10n ** 15n)
    expect(stats.totalData).toBe(12n)
    // expiresAt 100 is still active at anchor 50 (extensions_blocked or not);
    // expiresAt 10 is expired
    expect(stats.activeAgreements).toBe(2)
  })
})
