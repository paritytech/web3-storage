// SPDX-License-Identifier: GPL-3.0-only
//
// Photos' browser handle on the library bucket's file tree: a Web3Storage
// memoized on the chain api. The tree is stored in the bucket and read and
// written through the provider's blob routes only. The user's account
// (`setFsSigner`) signs the provider requests: writes need its Writer role,
// and finding the root of the private library bucket needs Reader. The
// client-side root recompute lives in `fs-root.ts` (shared with the headless
// scripts).

import { type ParachainApi } from '@web3-storage/papi'
import { Web3Storage, type ChainSigner, type FsTree } from '@web3-storage/sdk'
import { requireApi } from '@/lib/chain-client'

let storage: Web3Storage | null = null
let storageApi: ParachainApi | null = null
let fsSigner: ChainSigner | null = null

/** The shared Web3Storage, rebuilt if the chain api changes (e.g. network switch). */
function getStorage(): Web3Storage {
  const api = requireApi()
  if (!storage || storageApi !== api) {
    storage = new Web3Storage({ api, signer: fsSigner })
    storageApi = api
  }
  return storage
}

/** The file tree of `bucketId`, signed with the current account. */
export function getDrive(bucketId: bigint): Promise<FsTree> {
  return getStorage().drive(bucketId)
}

/** Set the account that signs provider requests, or `null` to clear it. */
export function setFsSigner(signer: ChainSigner | null): void {
  fsSigner = signer
  storage?.setSigner(signer)
}
