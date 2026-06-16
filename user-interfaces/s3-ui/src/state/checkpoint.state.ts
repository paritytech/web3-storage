/**
 * Checkpoint State - read-only snapshot of bucket checkpoint info + provider
 * duty status for the currently-selected bucket.
 */

import { BehaviorSubject } from "rxjs";
import { bind } from "@react-rxjs/core";
import type { CheckpointInfo, CheckpointDuty } from "@/lib/s3-client";
import { getS3Client } from "@/state/s3.state";

const checkpointInfo$ = new BehaviorSubject<CheckpointInfo | null>(null);
const checkpointDuty$ = new BehaviorSubject<CheckpointDuty | null>(null);
const checkpointLoading$ = new BehaviorSubject<boolean>(false);

export const [useCheckpointInfo] = bind(checkpointInfo$, null);
export const [useCheckpointDuty] = bind(checkpointDuty$, null);
export const [useCheckpointLoading] = bind(checkpointLoading$, false);

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

export async function triggerCheckpoint(bucketId: bigint): Promise<void> {
  const client = getS3Client();
  await client.triggerCheckpoint(bucketId);
  setTimeout(() => {
    refreshCheckpoint(bucketId).catch(() => {});
  }, 5000);
}

export function clearCheckpointState(): void {
  checkpointInfo$.next(null);
  checkpointDuty$.next(null);
}
