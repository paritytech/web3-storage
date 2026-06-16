/**
 * Challenge State - tracks the lifecycle of a storage provider challenge
 * for the currently-selected bucket, plus a list of all open challenges.
 *
 * Status lifecycle (for the current submission):
 *   idle → submitting → submitted → defended | slashed → idle
 *
 * After submission, subscribes to ChallengeDefended / ChallengeSlashed events
 * to detect the outcome with full details. Falls back to polling if the event
 * subscription misses the result (e.g. reconnect).
 */

import { BehaviorSubject } from "rxjs";
import { bind } from "@react-rxjs/core";
import type { OpenChallenge, ChallengeDefenseResult, ChallengeSlashResult } from "@/lib/s3-client";
import { getS3Client } from "@/state/s3.state";
import { getClient, getApi } from "@/state/chain.state";

export type ChallengeStatus = "idle" | "submitting" | "submitted" | "defended" | "slashed";

export interface ChallengeDefenseDetails {
  responseTimeBlocks: number;
  challengerCost: bigint;
  providerCost: bigint;
  blockNumber: number;
  blockHash: string;
}

export interface ChallengeSlashDetails {
  slashedAmount: bigint;
  challengerReward: bigint;
  blockNumber: number;
  blockHash: string;
}

export interface ActiveChallenge {
  challengeId: { deadline: number; index: number };
  respondBy: number;
  status: ChallengeStatus;
  bucketId: bigint;
  defenseDetails?: ChallengeDefenseDetails;
  slashDetails?: ChallengeSlashDetails;
}

export interface ChallengeHistoryEntry {
  challengeId: { deadline: number; index: number };
  bucketId: string;
  provider: string;
  status: "defended" | "slashed";
  blockNumber: number;
  blockHash: string;
  defenseDetails?: ChallengeDefenseDetails;
  slashDetails?: ChallengeSlashDetails;
}

// ── Subjects ────────────────────────────────────────────────────────────

const activeChallenge$ = new BehaviorSubject<ActiveChallenge | null>(null);
const challengeStatus$ = new BehaviorSubject<ChallengeStatus>("idle");
const openChallenges$ = new BehaviorSubject<OpenChallenge[]>([]);
const openChallengesLoading$ = new BehaviorSubject<boolean>(false);
const challengeHistory$ = new BehaviorSubject<ChallengeHistoryEntry[]>([]);
const challengeHistoryLoading$ = new BehaviorSubject<boolean>(false);
const showOutcomeModal$ = new BehaviorSubject<boolean>(false);

export const [useActiveChallenge] = bind(activeChallenge$, null);
export const [useChallengeStatus] = bind(challengeStatus$, "idle" as ChallengeStatus);
export const [useOpenChallenges] = bind(openChallenges$, []);
export const [useOpenChallengesLoading] = bind(openChallengesLoading$, false);
export const [useChallengeHistory] = bind(challengeHistory$, []);
export const [useChallengeHistoryLoading] = bind(challengeHistoryLoading$, false);
export const [useShowOutcomeModal] = bind(showOutcomeModal$, false);

// Per-challenge trackers keyed by "deadline:provider"
const eventWatchers = new Map<string, () => void>();
const pollTimers = new Map<string, ReturnType<typeof setTimeout>>();

const FALLBACK_POLL_MS = 300_000; // 5 minutes before fallback poll
const POLL_INTERVAL_MS = 6_000;

function watchKey(deadline: number, provider: string): string {
  return `${deadline}:${provider}`;
}

function stopPolling(key?: string): void {
  if (key) {
    const timer = pollTimers.get(key);
    if (timer !== undefined) {
      clearTimeout(timer);
      pollTimers.delete(key);
    }
  } else {
    for (const timer of pollTimers.values()) clearTimeout(timer);
    pollTimers.clear();
  }
}

function stopEventWatch(key?: string): void {
  if (key) {
    const unsub = eventWatchers.get(key);
    if (unsub) {
      unsub();
      eventWatchers.delete(key);
    }
  } else {
    for (const unsub of eventWatchers.values()) unsub();
    eventWatchers.clear();
  }
}

function setStatus(status: ChallengeStatus): void {
  challengeStatus$.next(status);
  const current = activeChallenge$.getValue();
  if (current) {
    activeChallenge$.next({ ...current, status });
  }
}

// ── Open challenges list ──────────────────────────────────────────────────

export async function refreshOpenChallenges(bucketId: bigint | null): Promise<void> {
  if (bucketId === null) {
    openChallenges$.next([]);
    return;
  }
  const client = getS3Client();
  if (!client.hasApi()) return;
  openChallengesLoading$.next(true);
  try {
    const challenges = await client.getOpenChallenges(bucketId);
    openChallenges$.next(challenges);
  } catch {
    // keep stale data on error
  } finally {
    openChallengesLoading$.next(false);
  }
}

// ── Submit + event watch ─────────────────────────────────────────────────

export async function submitChallenge(
  bucketId: bigint,
  provider: string,
  leafIndex: bigint,
  chunkIndex: bigint,
): Promise<void> {
  // Don't stop previous watchers — they track earlier challenges independently
  challengeStatus$.next("submitting");
  activeChallenge$.next({
    challengeId: { deadline: 0, index: 0 },
    respondBy: 0,
    status: "submitting",
    bucketId,
  });

  const client = getS3Client();
  try {
    const result = await client.challengeCheckpoint(bucketId, provider, leafIndex, chunkIndex);

    activeChallenge$.next({
      challengeId: result.challengeId,
      respondBy: result.respondBy,
      status: "submitted",
      bucketId,
    });
    challengeStatus$.next("submitted");

    // Refresh open challenges list after submission
    refreshOpenChallenges(bucketId).catch(() => {});

    // Subscribe to events for real-time outcome detection
    startEventWatch(result.challengeId, provider, bucketId);

    // Fallback: if no event fires within 5 min, poll to detect resolution
    startFallbackPoll(result.challengeId, provider, bucketId);
  } catch (err) {
    challengeStatus$.next("idle");
    activeChallenge$.next(null);
    throw err;
  }
}

function startEventWatch(
  challengeId: { deadline: number; index: number },
  provider: string,
  bucketId: bigint,
): void {
  const client = getS3Client();
  if (!client.hasApi()) return;

  const key = watchKey(challengeId.deadline, provider);

  const onDefended = (result: ChallengeDefenseResult): void => {
    stopPolling(key);
    stopEventWatch(key);
    const details: ChallengeDefenseDetails = {
      responseTimeBlocks: result.responseTimeBlocks,
      challengerCost: result.challengerCost,
      providerCost: result.providerCost,
      blockNumber: result.blockNumber,
      blockHash: result.blockHash,
    };
    // Update active challenge UI only if this is the one we're currently tracking
    const current = activeChallenge$.getValue();
    if (current && current.challengeId.deadline === challengeId.deadline
        && current.challengeId.index === challengeId.index) {
      activeChallenge$.next({ ...current, status: "defended", defenseDetails: details });
      challengeStatus$.next("defended");
      showOutcomeModal$.next(true);
    }
    refreshOpenChallenges(bucketId).catch(() => {});
    refreshChallengeHistory(bucketId).catch(() => {});
  };

  const onSlashed = (result: ChallengeSlashResult): void => {
    stopPolling(key);
    stopEventWatch(key);
    const details: ChallengeSlashDetails = {
      slashedAmount: result.slashedAmount,
      challengerReward: result.challengerReward,
      blockNumber: result.blockNumber,
      blockHash: result.blockHash,
    };
    const current = activeChallenge$.getValue();
    if (current && current.challengeId.deadline === challengeId.deadline
        && current.challengeId.index === challengeId.index) {
      activeChallenge$.next({ ...current, status: "slashed", slashDetails: details });
      challengeStatus$.next("slashed");
      showOutcomeModal$.next(true);
    }
    refreshOpenChallenges(bucketId).catch(() => {});
    refreshChallengeHistory(bucketId).catch(() => {});
  };

  const unsub = client.watchChallengeOutcome(challengeId.deadline, provider, onDefended, onSlashed);
  eventWatchers.set(key, unsub);
}

function startFallbackPoll(
  challengeId: { deadline: number; index: number },
  provider: string,
  bucketId: bigint,
): void {
  const key = watchKey(challengeId.deadline, provider);

  const timer = setTimeout(async () => {
    // Only poll if this challenge's event watcher is still active (not yet resolved)
    if (!eventWatchers.has(key)) return;

    const pollOnce = async (): Promise<boolean> => {
      try {
        const client = getS3Client();
        if (!client.hasApi()) return false;
        const stillActive = await client.isChallengeActive(challengeId.deadline);
        if (!stillActive) {
          stopEventWatch(key);
          stopPolling(key);
          // Update active challenge UI if this is the current one
          const current = activeChallenge$.getValue();
          if (current && current.challengeId.deadline === challengeId.deadline
              && current.challengeId.index === challengeId.index) {
            setStatus("defended");
            showOutcomeModal$.next(true);
          }
          refreshOpenChallenges(bucketId).catch(() => {});
          refreshChallengeHistory(bucketId).catch(() => {});
          return true; // resolved
        }
      } catch {
        // ignore transient errors
      }
      return false;
    };

    const resolved = await pollOnce();
    if (resolved) return;

    // If still unresolved after first poll, keep polling every 6s
    if (eventWatchers.has(key)) {
      const interval = setInterval(async () => {
        if (!eventWatchers.has(key)) {
          clearInterval(interval);
          pollTimers.delete(key);
          return;
        }
        const done = await pollOnce();
        if (done) clearInterval(interval);
      }, POLL_INTERVAL_MS);
      pollTimers.set(key, interval as unknown as ReturnType<typeof setTimeout>);
    }
  }, FALLBACK_POLL_MS);

  pollTimers.set(key, timer);
}

// ── Challenge history (chain scan + localStorage persistence) ────────────
//
// Strategy: scan recent finalized blocks (within pruning window ≈256) for
// challenge events, merge with localStorage-persisted history so data
// accumulates across sessions even after the chain prunes old state.

const SCAN_BATCH_SIZE = 20;
const MAX_SCAN_BLOCKS = 250; // stay within typical 256-block pruning window
const HISTORY_STORAGE_KEY = "s3-ui-challenge-history";

// ── localStorage helpers ────────────────────────────────────────────────

interface StoredHistoryEntry {
  challengeId: { deadline: number; index: number };
  bucketId: string;
  provider: string;
  status: "defended" | "slashed";
  blockNumber: number;
  blockHash: string;
  defenseDetails?: {
    responseTimeBlocks: number;
    challengerCost: string;   // bigint serialized as string
    providerCost: string;
    blockNumber: number;
    blockHash: string;
  };
  slashDetails?: {
    slashedAmount: string;    // bigint serialized as string
    challengerReward: string;
    blockNumber: number;
    blockHash: string;
  };
}

function serializeEntry(e: ChallengeHistoryEntry): StoredHistoryEntry {
  return {
    challengeId: e.challengeId,
    bucketId: e.bucketId,
    provider: e.provider,
    status: e.status,
    blockNumber: e.blockNumber,
    blockHash: e.blockHash,
    defenseDetails: e.defenseDetails ? {
      ...e.defenseDetails,
      challengerCost: e.defenseDetails.challengerCost.toString(),
      providerCost: e.defenseDetails.providerCost.toString(),
    } : undefined,
    slashDetails: e.slashDetails ? {
      ...e.slashDetails,
      slashedAmount: e.slashDetails.slashedAmount.toString(),
      challengerReward: e.slashDetails.challengerReward.toString(),
    } : undefined,
  };
}

function deserializeEntry(s: StoredHistoryEntry): ChallengeHistoryEntry {
  return {
    challengeId: s.challengeId,
    bucketId: s.bucketId,
    provider: s.provider,
    status: s.status,
    blockNumber: s.blockNumber,
    blockHash: s.blockHash,
    defenseDetails: s.defenseDetails ? {
      ...s.defenseDetails,
      challengerCost: BigInt(s.defenseDetails.challengerCost),
      providerCost: BigInt(s.defenseDetails.providerCost),
    } : undefined,
    slashDetails: s.slashDetails ? {
      ...s.slashDetails,
      slashedAmount: BigInt(s.slashDetails.slashedAmount),
      challengerReward: BigInt(s.slashDetails.challengerReward),
    } : undefined,
  };
}

function loadStoredHistory(): ChallengeHistoryEntry[] {
  try {
    const raw = localStorage.getItem(HISTORY_STORAGE_KEY);
    if (!raw) return [];
    const parsed: StoredHistoryEntry[] = JSON.parse(raw);
    return parsed.map(deserializeEntry);
  } catch {
    return [];
  }
}

function saveHistory(entries: ChallengeHistoryEntry[]): void {
  try {
    localStorage.setItem(HISTORY_STORAGE_KEY, JSON.stringify(entries.map(serializeEntry)));
  } catch {
    // storage full — ignore
  }
}

function historyKey(e: { challengeId: { deadline: number; index: number }; status: string }): string {
  return `${e.challengeId.deadline}:${e.challengeId.index}:${e.status}`;
}

function mergeHistory(existing: ChallengeHistoryEntry[], scanned: ChallengeHistoryEntry[]): ChallengeHistoryEntry[] {
  const seen = new Set(existing.map(historyKey));
  const merged = [...existing];
  for (const entry of scanned) {
    const key = historyKey(entry);
    if (!seen.has(key)) {
      merged.push(entry);
      seen.add(key);
    }
  }
  merged.sort((a, b) => b.blockNumber - a.blockNumber);
  return merged;
}

// Load persisted history on module init
const storedHistory = loadStoredHistory();
if (storedHistory.length > 0) {
  challengeHistory$.next(storedHistory);
}

// ── Chain scan ───────────────────────────────────────────────────────────

export async function refreshChallengeHistory(bucketId: bigint | null): Promise<void> {
  if (bucketId === null) {
    challengeHistory$.next([]);
    return;
  }

  const client = getClient();
  const api = getApi();
  if (!client || !api) return;

  challengeHistoryLoading$.next(true);
  try {
    const finalized = await client.getFinalizedBlock();
    const scanCount = Math.min(finalized.number, MAX_SCAN_BLOCKS);

    // Get block hashes in parallel using RPC
    const blockNumbers = Array.from({ length: scanCount }, (_, i) => finalized.number - i);
    const hashBatches: string[][] = [];
    for (let b = 0; b < blockNumbers.length; b += SCAN_BATCH_SIZE) {
      const batch = blockNumbers.slice(b, b + SCAN_BATCH_SIZE);
      const hashes = await Promise.all(
        batch.map((n) => client._request<string>("chain_getBlockHash", [n])),
      );
      hashBatches.push(hashes);
    }
    const blockHashes = hashBatches.flat();

    // Collect events — handle per-block errors (pruned state) gracefully
    type CreatedInfo = { challengeId: { deadline: number; index: number }; provider: string; bucketId: bigint };
    const createdMap = new Map<string, CreatedInfo>();

    type EventWithBlock = { payload: any; blockNumber: number; blockHash: string }; // eslint-disable-line @typescript-eslint/no-explicit-any
    const allDefended: EventWithBlock[] = [];
    const allSlashed: EventWithBlock[] = [];

    for (let b = 0; b < blockHashes.length; b += SCAN_BATCH_SIZE) {
      const batch = blockHashes.slice(b, b + SCAN_BATCH_SIZE);
      const batchStart = b;
      const results = await Promise.allSettled(
        batch.map(async (hash, i) => {
          const num = blockNumbers[batchStart + i]!;
          const [created, defended, slashed] = await Promise.all([
            api.event.StorageProvider.ChallengeCreated.get(hash),
            api.event.StorageProvider.ChallengeDefended.get(hash),
            api.event.StorageProvider.ChallengeSlashed.get(hash),
          ]);
          return { hash, number: num, created, defended, slashed };
        }),
      );

      for (const result of results) {
        if (result.status !== "fulfilled") continue; // skip pruned blocks
        const { hash, number, created, defended, slashed } = result.value;
        for (const ev of created) {
          const p = ev.payload;
          if (BigInt(p.bucket_id) !== bucketId) continue;
          const key = `${p.challenge_id.deadline}:${p.challenge_id.index}`;
          createdMap.set(key, {
            challengeId: { deadline: p.challenge_id.deadline, index: p.challenge_id.index },
            provider: p.provider,
            bucketId: BigInt(p.bucket_id),
          });
        }
        for (const ev of defended) {
          allDefended.push({ payload: ev.payload, blockNumber: number, blockHash: hash });
        }
        for (const ev of slashed) {
          allSlashed.push({ payload: ev.payload, blockNumber: number, blockHash: hash });
        }
      }
    }

    // Correlate: match defended/slashed events to challenges created for this bucket
    const scanned: ChallengeHistoryEntry[] = [];

    for (const { payload: p, blockNumber, blockHash } of allDefended) {
      const key = `${p.challenge_id.deadline}:${p.challenge_id.index}`;
      const info = createdMap.get(key);
      if (!info) continue;
      scanned.push({
        challengeId: info.challengeId,
        bucketId: info.bucketId.toString(),
        provider: info.provider,
        status: "defended",
        blockNumber,
        blockHash,
        defenseDetails: {
          responseTimeBlocks: p.response_time_blocks,
          challengerCost: p.challenger_cost,
          providerCost: p.provider_cost,
          blockNumber,
          blockHash,
        },
      });
    }

    for (const { payload: p, blockNumber, blockHash } of allSlashed) {
      const key = `${p.challenge_id.deadline}:${p.challenge_id.index}`;
      const info = createdMap.get(key);
      if (!info) continue;
      scanned.push({
        challengeId: info.challengeId,
        bucketId: info.bucketId.toString(),
        provider: info.provider,
        status: "slashed",
        blockNumber,
        blockHash,
        slashDetails: {
          slashedAmount: p.slashed_amount,
          challengerReward: p.challenger_reward,
          blockNumber,
          blockHash,
        },
      });
    }

    // Merge chain-scanned entries with localStorage-persisted history
    const existing = loadStoredHistory();
    const merged = mergeHistory(existing, scanned);
    saveHistory(merged);

    // Filter for the requested bucket and publish
    const forBucket = merged.filter((e) => e.bucketId === bucketId.toString());
    challengeHistory$.next(forBucket);
  } catch (err) {
    console.error("[challenge-history] scan failed:", err);
  } finally {
    challengeHistoryLoading$.next(false);
  }
}

export function clearChallengeHistory(): void {
  challengeHistory$.next([]);
  try { localStorage.removeItem(HISTORY_STORAGE_KEY); } catch { /* ignore */ }
}

// ── Dismiss / clear ──────────────────────────────────────────────────────

export function dismissChallengeResult(): void {
  showOutcomeModal$.next(false);
  activeChallenge$.next(null);
  challengeStatus$.next("idle");
}

export function clearChallengeState(): void {
  stopPolling();      // stops all poll timers
  stopEventWatch();   // stops all event watchers
  showOutcomeModal$.next(false);
  activeChallenge$.next(null);
  challengeStatus$.next("idle");
}
