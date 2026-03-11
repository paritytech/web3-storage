import { vi, describe, it, expect, beforeEach, afterEach } from 'vitest'

// ── Mock setup (must precede module-under-test import) ───────────────────────

const mockGetEntries = {
  agreements: vi.fn(),
  requests: vi.fn(),
  challenges: vi.fn(),
}

const mockGetValue = {
  providers: vi.fn(),
  snapshots: vi.fn(),
  account: vi.fn(),
}

const mockUnsafeApi = {
  query: {
    StorageProvider: {
      StorageAgreements: { getEntries: mockGetEntries.agreements },
      AgreementRequests: { getEntries: mockGetEntries.requests },
      Challenges: { getEntries: mockGetEntries.challenges },
      Providers: { getValue: mockGetValue.providers },
      BucketSnapshots: { getValue: mockGetValue.snapshots },
    },
    System: {
      Account: { getValue: mockGetValue.account },
    },
  },
}

const mockClient = {
  getUnsafeApi: () => mockUnsafeApi,
  bestBlocks$: { subscribe: vi.fn() },
  finalizedBlock$: { subscribe: vi.fn() },
  destroy: vi.fn(),
}

vi.mock('polkadot-api', () => ({
  createClient: () => mockClient,
}))

vi.mock('polkadot-api/ws-provider/web', () => ({
  getWsProvider: () => ({}),
}))

vi.mock('polkadot-api/pjs-signer', () => ({}))

vi.mock('@polkadot/api', () => ({
  ApiPromise: {
    create: () =>
      Promise.resolve({
        disconnect: vi.fn(),
        registry: { findMetaError: vi.fn() },
        tx: {},
      }),
  },
  WsProvider: vi.fn().mockImplementation(() => ({
    disconnect: vi.fn(),
  })),
}))

vi.mock('@polkadot/keyring', () => ({
  Keyring: vi.fn(),
}))

// ── Import module under test ─────────────────────────────────────────────────

import {
  connectToChain,
  disconnectFromChain,
  blockNumber$,
  getProviderAgreements,
  getAgreementRequests,
  getProviderChallenges,
  getProviderInfo,
  getProviderSettings,
} from './chain-client'

// ── Test helpers ─────────────────────────────────────────────────────────────

const PROVIDER = '5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY'
const OTHER_PROVIDER = '5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty'
const USER = '5DAAnrj7VHTznn2AWBemMuyBwZWs6FNFjdyVXUeYum3PTXFy'

/** Build a StorageAgreements entry in polkadot-api { keyArgs, value } format */
function agreementEntry(
  bucketId: number,
  provider: string,
  overrides: Record<string, unknown> = {},
) {
  return {
    keyArgs: [bucketId, provider],
    value: {
      owner: USER,
      max_bytes: 1_000_000n,
      price_per_byte: 100n,
      payment_locked: 50_000_000n,
      started_at: 100,
      expires_at: 1000,
      extensions_blocked: false,
      role: { type: 'Primary' },
      ...overrides,
    },
  }
}

/** Build an AgreementRequests entry */
function requestEntry(
  provider: string,
  bucketId: number,
  overrides: Record<string, unknown> = {},
) {
  return {
    keyArgs: [provider, bucketId],
    value: {
      requester: USER,
      max_bytes: 500_000n,
      payment_locked: 25_000_000n,
      duration: 200,
      expires_at: 700,
      ...overrides,
    },
  }
}

/** Build a Challenges entry */
function challengeEntry(
  challengeId: number,
  overrides: Record<string, unknown> = {},
) {
  return {
    keyArgs: [challengeId],
    value: {
      bucket_id: 1,
      challenger: USER,
      provider: PROVIDER,
      leaf_index: 42,
      responded: false,
      slashed: false,
      created_at: 300,
      deadline: 600,
      ...overrides,
    },
  }
}

// ── Tests ────────────────────────────────────────────────────────────────────

describe('chain-client queries', () => {
  beforeEach(async () => {
    vi.spyOn(console, 'log').mockImplementation(() => {})
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    vi.spyOn(console, 'error').mockImplementation(() => {})

    await connectToChain('ws://mock:9944')
    blockNumber$.next(500)

    // Clear recorded calls from connection setup; mock implementations persist
    vi.clearAllMocks()
  })

  afterEach(() => {
    disconnectFromChain()
    vi.restoreAllMocks()
  })

  // ── getProviderAgreements ────────────────────────────────────────────

  describe('getProviderAgreements', () => {
    it('parses { keyArgs, value } entries correctly', async () => {
      mockGetEntries.agreements.mockResolvedValue([agreementEntry(1, PROVIDER)])

      const result = await getProviderAgreements(PROVIDER)

      expect(result).toHaveLength(1)
      expect(result[0]).toMatchObject({
        bucketId: 1,
        provider: PROVIDER,
        user: USER,
        maxBytes: 1_000_000n,
        pricePerByte: 100n,
        paymentLocked: 50_000_000n,
        startBlock: 100,
        endBlock: 1000,
        isPrimary: true,
        status: 'active',
      })
    })

    it('extracts bucketId as number, not bigint', async () => {
      mockGetEntries.agreements.mockResolvedValue([agreementEntry(42, PROVIDER)])

      const result = await getProviderAgreements(PROVIDER)

      expect(result[0]!.bucketId).toBe(42)
      expect(typeof result[0]!.bucketId).toBe('number')
    })

    it('filters entries by provider address', async () => {
      mockGetEntries.agreements.mockResolvedValue([
        agreementEntry(1, PROVIDER),
        agreementEntry(2, OTHER_PROVIDER),
        agreementEntry(3, PROVIDER),
      ])

      const result = await getProviderAgreements(PROVIDER)

      expect(result).toHaveLength(2)
      expect(result.map((a) => a.bucketId)).toEqual([1, 3])
    })

    it('marks status as "terminated" when extensions_blocked is true', async () => {
      mockGetEntries.agreements.mockResolvedValue([
        agreementEntry(1, PROVIDER, { extensions_blocked: true }),
      ])

      const result = await getProviderAgreements(PROVIDER)

      expect(result[0]!.status).toBe('terminated')
    })

    it('marks status as "expired" when current block exceeds expires_at', async () => {
      blockNumber$.next(1500) // past expires_at=1000

      mockGetEntries.agreements.mockResolvedValue([
        agreementEntry(1, PROVIDER, { expires_at: 1000 }),
      ])

      const result = await getProviderAgreements(PROVIDER)

      expect(result[0]!.status).toBe('expired')
    })

    it('marks status as "active" when block is before expires_at', async () => {
      blockNumber$.next(500)

      mockGetEntries.agreements.mockResolvedValue([
        agreementEntry(1, PROVIDER, { expires_at: 1000 }),
      ])

      const result = await getProviderAgreements(PROVIDER)

      expect(result[0]!.status).toBe('active')
    })

    it('parses role via role.type', async () => {
      mockGetEntries.agreements.mockResolvedValue([
        agreementEntry(1, PROVIDER, { role: { type: 'Replica' } }),
      ])

      const result = await getProviderAgreements(PROVIDER)

      expect(result[0]!.isPrimary).toBe(false)
    })

    it('parses role via role.tag', async () => {
      mockGetEntries.agreements.mockResolvedValue([
        agreementEntry(1, PROVIDER, { role: { tag: 'Primary' } }),
      ])

      const result = await getProviderAgreements(PROVIDER)

      expect(result[0]!.isPrimary).toBe(true)
    })

    it('parses role as plain string', async () => {
      mockGetEntries.agreements.mockResolvedValue([
        agreementEntry(1, PROVIDER, { role: 'Primary' }),
      ])

      const result = await getProviderAgreements(PROVIDER)

      expect(result[0]!.isPrimary).toBe(true)
    })

    it('defaults to primary when role has no type or tag', async () => {
      mockGetEntries.agreements.mockResolvedValue([
        agreementEntry(1, PROVIDER, { role: {} }),
      ])

      const result = await getProviderAgreements(PROVIDER)

      expect(result[0]!.isPrimary).toBe(true)
    })

    it('returns [] for empty entries', async () => {
      mockGetEntries.agreements.mockResolvedValue([])

      const result = await getProviderAgreements(PROVIDER)

      expect(result).toEqual([])
    })

    it('skips entries with null value', async () => {
      mockGetEntries.agreements.mockResolvedValue([
        { keyArgs: [1, PROVIDER], value: null },
        agreementEntry(2, PROVIDER),
      ])

      const result = await getProviderAgreements(PROVIDER)

      expect(result).toHaveLength(1)
      expect(result[0]!.bucketId).toBe(2)
    })
  })

  // ── getAgreementRequests ─────────────────────────────────────────────

  describe('getAgreementRequests', () => {
    it('parses { keyArgs, value } entries correctly', async () => {
      mockGetEntries.requests.mockResolvedValue([requestEntry(PROVIDER, 5)])

      const result = await getAgreementRequests(PROVIDER)

      expect(result).toHaveLength(1)
      expect(result[0]).toMatchObject({
        bucketId: 5,
        requester: USER,
        maxBytes: 500_000n,
        paymentLocked: 25_000_000n,
        duration: 200,
        expiresAt: 700,
      })
    })

    it('filters entries by provider address', async () => {
      mockGetEntries.requests.mockResolvedValue([
        requestEntry(PROVIDER, 1),
        requestEntry(OTHER_PROVIDER, 2),
        requestEntry(PROVIDER, 3),
      ])

      const result = await getAgreementRequests(PROVIDER)

      expect(result).toHaveLength(2)
      expect(result.map((r) => r.bucketId)).toEqual([1, 3])
    })

    it('returns [] for empty entries', async () => {
      mockGetEntries.requests.mockResolvedValue([])

      const result = await getAgreementRequests(PROVIDER)

      expect(result).toEqual([])
    })

    it('skips entries with null value', async () => {
      mockGetEntries.requests.mockResolvedValue([
        { keyArgs: [PROVIDER, 1], value: null },
        requestEntry(PROVIDER, 2),
      ])

      const result = await getAgreementRequests(PROVIDER)

      expect(result).toHaveLength(1)
      expect(result[0]!.bucketId).toBe(2)
    })
  })

  // ── getProviderChallenges ────────────────────────────────────────────

  describe('getProviderChallenges', () => {
    it('parses { keyArgs, value } entries correctly', async () => {
      mockGetEntries.challenges.mockResolvedValue([challengeEntry(10)])

      const result = await getProviderChallenges(PROVIDER)

      expect(result).toHaveLength(1)
      expect(result[0]).toMatchObject({
        id: 10,
        bucketId: 1,
        challenger: USER,
        provider: PROVIDER,
        leafIndex: 42,
        status: 'pending',
        createdAt: 300,
        deadline: 600,
      })
    })

    it('marks status as "responded" when responded=true', async () => {
      mockGetEntries.challenges.mockResolvedValue([
        challengeEntry(1, { responded: true }),
      ])

      const result = await getProviderChallenges(PROVIDER)

      expect(result[0]!.status).toBe('responded')
    })

    it('marks status as "slashed" when slashed=true', async () => {
      mockGetEntries.challenges.mockResolvedValue([
        challengeEntry(1, { slashed: true }),
      ])

      const result = await getProviderChallenges(PROVIDER)

      expect(result[0]!.status).toBe('slashed')
    })

    it('marks status as "expired" when current block > deadline', async () => {
      blockNumber$.next(700) // past deadline=600

      mockGetEntries.challenges.mockResolvedValue([challengeEntry(1)])

      const result = await getProviderChallenges(PROVIDER)

      expect(result[0]!.status).toBe('expired')
    })

    it('filters by provider in value.provider', async () => {
      mockGetEntries.challenges.mockResolvedValue([
        challengeEntry(1, { provider: PROVIDER }),
        challengeEntry(2, { provider: OTHER_PROVIDER }),
      ])

      const result = await getProviderChallenges(PROVIDER)

      expect(result).toHaveLength(1)
      expect(result[0]!.id).toBe(1)
    })

    it('returns [] for empty entries', async () => {
      mockGetEntries.challenges.mockResolvedValue([])

      const result = await getProviderChallenges(PROVIDER)

      expect(result).toEqual([])
    })
  })

  // ── getProviderInfo ──────────────────────────────────────────────────

  describe('getProviderInfo', () => {
    it('returns mapped provider info from getValue', async () => {
      mockGetValue.providers.mockResolvedValue({
        stake: 1_000_000_000_000n,
        settings: { max_capacity: 10_737_418_240n },
        committed_bytes: 5_000_000n,
        stats: { agreements_total: 3, registered_at: 100 },
      })

      const result = await getProviderInfo(PROVIDER)

      expect(result).toMatchObject({
        stake: 1_000_000_000_000n,
        capacity: 10_737_418_240n,
        usedCapacity: 5_000_000n,
        activeBuckets: 3,
        registeredAt: 100,
      })
    })

    it('returns null when getValue returns undefined', async () => {
      mockGetValue.providers.mockResolvedValue(undefined)

      const result = await getProviderInfo(PROVIDER)

      expect(result).toBeNull()
    })

    it('returns null when getValue returns null', async () => {
      mockGetValue.providers.mockResolvedValue(null)

      const result = await getProviderInfo(PROVIDER)

      expect(result).toBeNull()
    })
  })

  // ── getProviderSettings ──────────────────────────────────────────────

  describe('getProviderSettings', () => {
    it('maps all settings fields from snake_case to camelCase', async () => {
      mockGetValue.providers.mockResolvedValue({
        settings: {
          min_duration: 100,
          max_duration: 5000,
          price_per_byte: 50n,
          accepting_primary: true,
          accepting_replica: false,
          replica_sync_price: null,
          accepting_extensions: true,
          max_capacity: 1_073_741_824n,
        },
      })

      const result = await getProviderSettings(PROVIDER)

      expect(result).toMatchObject({
        minDuration: 100,
        maxDuration: 5000,
        pricePerByte: 50n,
        acceptingPrimary: true,
        acceptingReplica: false,
        replicaSyncPrice: null,
        acceptingExtensions: true,
        maxCapacity: 1_073_741_824n,
      })
    })

    it('returns null when no provider found', async () => {
      mockGetValue.providers.mockResolvedValue(undefined)

      const result = await getProviderSettings(PROVIDER)

      expect(result).toBeNull()
    })

    it('returns null when provider has no settings', async () => {
      mockGetValue.providers.mockResolvedValue({ settings: null })

      const result = await getProviderSettings(PROVIDER)

      expect(result).toBeNull()
    })
  })

  // ── Error handling ───────────────────────────────────────────────────

  describe('error handling', () => {
    it('getProviderAgreements returns [] when getEntries throws', async () => {
      mockGetEntries.agreements.mockRejectedValue(new Error('network error'))

      const result = await getProviderAgreements(PROVIDER)

      expect(result).toEqual([])
    })

    it('getAgreementRequests returns [] when getEntries throws', async () => {
      mockGetEntries.requests.mockRejectedValue(new Error('network error'))

      const result = await getAgreementRequests(PROVIDER)

      expect(result).toEqual([])
    })

    it('getProviderChallenges returns [] when getEntries throws', async () => {
      mockGetEntries.challenges.mockRejectedValue(new Error('network error'))

      const result = await getProviderChallenges(PROVIDER)

      expect(result).toEqual([])
    })

    it('getProviderInfo returns null when getValue throws', async () => {
      mockGetValue.providers.mockRejectedValue(new Error('network error'))

      const result = await getProviderInfo(PROVIDER)

      expect(result).toBeNull()
    })

    it('getProviderSettings returns null when getValue throws', async () => {
      mockGetValue.providers.mockRejectedValue(new Error('network error'))

      const result = await getProviderSettings(PROVIDER)

      expect(result).toBeNull()
    })
  })

  // ── Not-connected guard ──────────────────────────────────────────────

  describe('not-connected guard', () => {
    beforeEach(() => {
      disconnectFromChain()
    })

    it('getProviderAgreements throws when not connected', async () => {
      await expect(getProviderAgreements(PROVIDER)).rejects.toThrow('Not connected')
    })

    it('getAgreementRequests throws when not connected', async () => {
      await expect(getAgreementRequests(PROVIDER)).rejects.toThrow('Not connected')
    })

    it('getProviderChallenges throws when not connected', async () => {
      await expect(getProviderChallenges(PROVIDER)).rejects.toThrow('Not connected')
    })

    it('getProviderInfo throws when not connected', async () => {
      await expect(getProviderInfo(PROVIDER)).rejects.toThrow('Not connected')
    })

    it('getProviderSettings throws when not connected', async () => {
      await expect(getProviderSettings(PROVIDER)).rejects.toThrow('Not connected')
    })
  })

  // ── Entry format regression guard ────────────────────────────────────

  describe('entry format regression guard', () => {
    it('{ keyArgs, value } format is parsed correctly (polkadot-api contract)', async () => {
      const entry = {
        keyArgs: [1, PROVIDER],
        value: {
          owner: USER,
          max_bytes: 100n,
          price_per_byte: 1n,
          payment_locked: 50n,
          started_at: 10,
          expires_at: 1000,
          extensions_blocked: false,
          role: { type: 'Primary' },
        },
      }

      mockGetEntries.agreements.mockResolvedValue([entry])

      const result = await getProviderAgreements(PROVIDER)

      expect(result).toHaveLength(1)
      expect(result[0]!.bucketId).toBe(1)
      expect(result[0]!.provider).toBe(PROVIDER)
    })

    it('array indexing on { keyArgs, value } objects gives undefined (documents the bug)', () => {
      // polkadot-api getEntries() returns { keyArgs, value } objects, NOT arrays.
      // The original bug: array-style access (entry[0], entry[1]) yields undefined,
      // silently producing empty results swallowed by catch blocks.
      const entry: Record<string, unknown> = {
        keyArgs: [1, PROVIDER],
        value: { owner: USER },
      }

      // Array-style access (the bug)
      expect((entry as any)[0]).toBeUndefined()
      expect((entry as any)[1]).toBeUndefined()

      // Object property access (the fix)
      expect(entry.keyArgs).toEqual([1, PROVIDER])
      expect(entry.value).toEqual({ owner: USER })
    })
  })
})
