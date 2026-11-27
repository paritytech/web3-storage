// SPDX-License-Identifier: GPL-3.0-only
//
// Photos' browser handle on the shared FileSystemClient (@web3-storage/sdk/fs):
// a client memoized on the chain api. The client keeps the directory tree in
// the library's bucket and talks to the provider's Layer 0 routes only.
// Reads need no signer. Writes (mkdir / put / delete) sign the provider
// requests with the user's account, which holds a Writer role on the bucket
// (`setFsSigner`). The client-side root recompute lives in `fs-root.ts`
// (shared with the headless scripts).

import { type ParachainApi } from '@web3-storage/papi'
import type { ChainSigner } from '@web3-storage/sdk'
import { FileSystemClient } from '@web3-storage/sdk/fs'
import { requireApi } from '@/lib/chain-client'

let fsClient: FileSystemClient | null = null
let fsClientApi: ParachainApi | null = null
let fsSigner: ChainSigner | null = null

/** Shared FileSystemClient, rebuilt if the chain api changes (e.g. network switch). */
export function getFsClient(): FileSystemClient {
  const api = requireApi()
  if (!fsClient || fsClientApi !== api) {
    fsClient = new FileSystemClient({ api, signer: fsSigner })
    fsClientApi = api
  }
  return fsClient
}

/** Set the account that signs write requests, or `null` to clear it. */
export function setFsSigner(signer: ChainSigner | null): void {
  fsSigner = signer
  fsClient?.setSigner(signer)
}
