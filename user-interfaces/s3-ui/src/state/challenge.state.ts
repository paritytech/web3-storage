/**
 * Challenge State - tracks the lifecycle of a storage provider challenge
 * for the currently-selected bucket, plus a list of all open challenges.
 *
 * Status lifecycle (for the current submission):
 *   idle → submitting → submitted → defended | slashed → idle
 *
 * After submission, polls the chain to detect whether the provider defended
 * or the challenge deadline passed (slashed).
 */

import { BehaviorSubject } from "rxjs";
import { bind } from "@react-rxjs/core";
import type { OpenChallenge } from "@/lib/s3-client";
import { getS3Client } from "@/state/s3.state";

export type ChallengeStatus = "idle" | "submitting" | "submitted" | "defended" | "slashed";

export interface ActiveChallenge {
  challengeId: { deadline: number; index: number };
  respondBy: number;
  status: ChallengeStatus;
  bucketId: bigint;
}

const activeChallenge$ = new BehaviorSubject<ActiveChallenge | null>(null);
const challengeStatus$ = new BehaviorSubject<ChallengeStatus>("idle");
const openChallenges$ = new BehaviorSubject<OpenChallenge[]>([]);
const openChallengesLoading$ = new BehaviorSubject<boolean>(false);

export const [useActiveChallenge] = bind(activeChallenge$, null);
export const [useChallengeStatus] = bind(challengeStatus$, "idle" as ChallengeStatus);
export const [useOpenChallenges] = bind(openChallenges$, []);
export const [useOpenChallengesLoading] = bind(openChallengesLoading$, false);

let pollTimer: ReturnType<typeof setInterval> | null = null;

const POLL_INTERVAL_MS = 6_000;
const POLL_MAX_MS = 300_000; // 5 minutes max polling
const RESULT_DISPLAY_MS = 6_000;

function stopPolling(): void {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
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

// ── Submit + poll ─────────────────────────────────────────────────────────

export async function submitChallenge(
  bucketId: bigint,
  provider: string,
  leafIndex: bigint,
  chunkIndex: bigint,
): Promise<void> {
  stopPolling();
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

    // Start polling to detect defense or slashing
    startPolling(result.challengeId.deadline, bucketId);
  } catch (err) {
    challengeStatus$.next("idle");
    activeChallenge$.next(null);
    throw err;
  }
}

function startPolling(deadline: number, bucketId: bigint): void {
  const startedAt = Date.now();

  pollTimer = setInterval(async () => {
    try {
      const client = getS3Client();
      if (!client.hasApi()) return;

      const stillActive = await client.isChallengeActive(deadline);

      if (!stillActive) {
        // Challenge was removed from storage — provider defended
        stopPolling();
        setStatus("defended");
        refreshOpenChallenges(bucketId).catch(() => {});
        setTimeout(() => {
          if (challengeStatus$.getValue() === "defended") {
            clearChallengeState();
          }
        }, RESULT_DISPLAY_MS);
        return;
      }
    } catch {
      // Ignore transient errors during polling
    }

    // Check if deadline has likely passed (rough heuristic: poll timeout)
    if (Date.now() - startedAt > POLL_MAX_MS) {
      stopPolling();
      try {
        const client = getS3Client();
        const stillActive = await client.isChallengeActive(deadline);
        if (!stillActive) {
          setStatus("slashed");
          refreshOpenChallenges(bucketId).catch(() => {});
          setTimeout(() => {
            if (challengeStatus$.getValue() === "slashed") {
              clearChallengeState();
            }
          }, RESULT_DISPLAY_MS);
          return;
        }
      } catch {
        // fall through
      }
    }
  }, POLL_INTERVAL_MS);
}

export function clearChallengeState(): void {
  stopPolling();
  activeChallenge$.next(null);
  challengeStatus$.next("idle");
}
