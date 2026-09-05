// SPDX-License-Identifier: GPL-3.0-only

/**
 * Checkpoint State - read-only snapshot of bucket checkpoint info for the
 * currently-selected bucket.
 */

import { BehaviorSubject } from "rxjs";
import { bind } from "@react-rxjs/core";
import type { CheckpointInfo } from "@/lib/s3-client";
import { getS3Client } from "@/state/s3.state";

const checkpointInfo$ = new BehaviorSubject<CheckpointInfo | null>(null);
const checkpointLoading$ = new BehaviorSubject<boolean>(false);

export const [useCheckpointInfo] = bind(checkpointInfo$, null);
export const [useCheckpointLoading] = bind(checkpointLoading$, false);

export async function refreshCheckpoint(bucketId: bigint | null): Promise<void> {
  if (bucketId === null) {
    checkpointInfo$.next(null);
    return;
  }
  const client = getS3Client();
  if (!client.hasApi()) return;
  checkpointLoading$.next(true);
  try {
    const info = await client.getCheckpointInfo(bucketId).catch(() => null);
    checkpointInfo$.next(info);
  } finally {
    checkpointLoading$.next(false);
  }
}

export function clearCheckpointState(): void {
  checkpointInfo$.next(null);
}
