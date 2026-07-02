// SPDX-License-Identifier: Apache-2.0

/**
 * Checkpoint State - read-only snapshot of bucket checkpoint info + provider
 * duty status for the currently-selected drive.
 */

import { BehaviorSubject } from "rxjs";
import { bind } from "@react-rxjs/core";
import type { CheckpointInfo, CheckpointDuty } from "@/lib/drive-client";
import { getDriveClient } from "@/state/drive.state";

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
  const client = getDriveClient();
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
  const client = getDriveClient();
  await client.triggerCheckpoint(bucketId);
  // Provider needs a moment to process; refresh shortly after
  setTimeout(() => {
    refreshCheckpoint(bucketId).catch(() => { /* swallow */ });
  }, 5000);
}

export function clearCheckpointState(): void {
  checkpointInfo$.next(null);
  checkpointDuty$.next(null);
}
