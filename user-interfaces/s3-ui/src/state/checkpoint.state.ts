/**
 * Checkpoint State - read-only snapshot of bucket checkpoint info + provider
 * duty status for the currently-selected bucket.
 *
 * After a checkpoint is triggered, the status tracks the lifecycle:
 *   idle → triggering → polling → confirmed → idle
 */

import { BehaviorSubject } from "rxjs";
import { bind } from "@react-rxjs/core";
import type { CheckpointInfo, CheckpointDuty } from "@/lib/s3-client";
import { getS3Client } from "@/state/s3.state";

export type CheckpointStatus = "idle" | "triggering" | "polling" | "confirmed";

const checkpointInfo$ = new BehaviorSubject<CheckpointInfo | null>(null);
const checkpointDuty$ = new BehaviorSubject<CheckpointDuty | null>(null);
const checkpointLoading$ = new BehaviorSubject<boolean>(false);
const checkpointStatus$ = new BehaviorSubject<CheckpointStatus>("idle");

export const [useCheckpointInfo] = bind(checkpointInfo$, null);
export const [useCheckpointDuty] = bind(checkpointDuty$, null);
export const [useCheckpointLoading] = bind(checkpointLoading$, false);
export const [useCheckpointStatus] = bind(checkpointStatus$, "idle" as CheckpointStatus);

let pollTimer: ReturnType<typeof setInterval> | null = null;

function stopPolling(): void {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

export async function refreshCheckpoint(bucketId: bigint | null): Promise<void> {
  if (bucketId === null) {
    checkpointInfo$.next(null);
    checkpointDuty$.next(null);
    return;
  }
  const client = getS3Client();
  if (!client.hasApi()) return;
  checkpointLoading$.next(true);
  try {
    const [info, duty] = await Promise.all([
      client.getCheckpointInfo(bucketId).catch(() => null),
      client.getCheckpointDuty(bucketId).catch(() => null),
    ]);
    checkpointInfo$.next(info);
    checkpointDuty$.next(duty);
  } finally {
    checkpointLoading$.next(false);
  }
}

const POLL_INTERVAL_MS = 4_000;
const POLL_MAX_MS = 120_000;
const CONFIRMED_DISPLAY_MS = 4_000;

export async function triggerCheckpoint(bucketId: bigint): Promise<void> {
  stopPolling();
  checkpointStatus$.next("triggering");

  const client = getS3Client();
  try {
    await client.triggerCheckpoint(bucketId);
  } catch (err) {
    checkpointStatus$.next("idle");
    throw err;
  }

  // Capture the block number before trigger so we can detect a new checkpoint
  const blockBefore = checkpointInfo$.getValue()?.checkpointBlock ?? -1;

  checkpointStatus$.next("polling");
  const startedAt = Date.now();

  pollTimer = setInterval(async () => {
    try {
      await refreshCheckpoint(bucketId);
      const current = checkpointInfo$.getValue();
      const newBlock = current?.checkpointBlock ?? -1;
      if (newBlock > blockBefore) {
        // New checkpoint landed on-chain
        stopPolling();
        checkpointStatus$.next("confirmed");
        setTimeout(() => {
          if (checkpointStatus$.getValue() === "confirmed") {
            checkpointStatus$.next("idle");
          }
        }, CONFIRMED_DISPLAY_MS);
      }
    } catch {
      // Ignore transient errors during polling
    }

    if (Date.now() - startedAt > POLL_MAX_MS) {
      stopPolling();
      checkpointStatus$.next("idle");
    }
  }, POLL_INTERVAL_MS);
}

export function clearCheckpointState(): void {
  stopPolling();
  checkpointInfo$.next(null);
  checkpointDuty$.next(null);
  checkpointStatus$.next("idle");
}
