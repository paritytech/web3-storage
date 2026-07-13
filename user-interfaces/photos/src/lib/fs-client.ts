// SPDX-License-Identifier: GPL-3.0-only
//
// Photos' browser handle on the shared FileSystemClient (@web3-storage/sdk/fs):
// a client memoized on the chain api (no signer → no auth headers, matching the
// dev provider's auth-disabled /fs), plus a driveId → bucketId resolver. All /fs
// ops (mkdir / put / download / ls / delete) are called on the client directly
// at the use sites; the client-side root recompute lives in `fs-root.ts` (shared
// with the headless scripts).

import { type ParachainApi } from '@web3-storage/papi'
import { FileSystemClient } from '@web3-storage/sdk/fs'
import { requireApi } from '@/lib/chain-client'

let fsClient: FileSystemClient | null = null
let fsClientApi: ParachainApi | null = null

/** Shared FileSystemClient, rebuilt if the chain api changes (e.g. network switch). */
export function getFsClient(): FileSystemClient {
  const api = requireApi()
  if (!fsClient || fsClientApi !== api) {
    fsClient = new FileSystemClient({ api })
    fsClientApi = api
  }
  return fsClient
}

/** Resolve a drive's bucket id from chain state (its `/fs` calls are keyed on it). */
export async function resolveBucketId(driveId: bigint): Promise<bigint> {
  const drive = await getFsClient().getDrive(driveId)
  if (!drive) throw new Error(`Drive ${driveId} not found on chain`)
  return drive.bucketId
}
