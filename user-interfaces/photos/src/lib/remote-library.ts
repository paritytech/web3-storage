// SPDX-License-Identifier: GPL-3.0-only
//
// Photos' library as files in its own remote storage bucket. The TrUAPI host
// owns the bucket, the file tree, the provider and the agreement, so photos
// only stores, reads, lists and removes paths, and asks for a plan. The host
// reports times in seconds since the Unix epoch.

import {
  storeFile,
  type CallErrorValue,
  type RemoteStoragePlan,
  type RemoteStorageQuote,
} from '@parity/truapi'
import { getClientSync } from '@parity/truapi/sandbox'
import { fromHex } from 'polkadot-api/utils'

/** A file in the library. */
export interface LibraryFile {
  /** Absolute path, e.g. `/Beach/photo.jpg`. */
  path: string
  /** Size in bytes. */
  size: number
  /** Last stored, in milliseconds since the Unix epoch. */
  modified: number
}

/** A remote storage call that failed, with the error the host named. */
export class RemoteStorageError extends Error {
  /** The domain error's tag, such as `NotFound`, or the call error's tag otherwise. */
  readonly code: string
  /** The host's price, for `PriceExceeded`. */
  readonly quoted?: bigint

  constructor(method: string, error: CallErrorValue<unknown>) {
    super(`remoteStorage.${method} failed: ${describe(error)}`)
    this.name = 'RemoteStorageError'
    const domain = error.tag === 'Domain' ? domainError(error.value) : undefined
    this.code = domain?.tag ?? error.tag
    if (domain?.tag === 'PriceExceeded') this.quoted = (domain.value as { quoted: bigint }).quoted
  }
}

/** Every file in the library, read one page of the listing at a time. */
export async function listFiles(): Promise<LibraryFile[]> {
  const files: LibraryFile[] = []
  let startAfter: string | undefined
  do {
    const result = await remoteStorage().list({ recursive: true, startAfter })
    if (result.isErr()) throw new RemoteStorageError('list', result.error)
    for (const entry of result.value.entries) {
      if (entry.kind.tag !== 'File') continue
      files.push({
        path: entry.path,
        size: Number(entry.kind.value.size),
        modified: Number(entry.modified) * 1000,
      })
    }
    const next = result.value.nextStartAfter
    // A page that names a next page but returns nothing new would repeat forever.
    if (next !== undefined && (result.value.entries.length === 0 || next === startAfter)) {
      throw new Error(`remoteStorage.list returned a page that does not advance past ${startAfter ?? '/'}`)
    }
    startAfter = next
  } while (startAfter !== undefined)
  return files
}

/** The bytes of the file at `path`, each chunk verified by the host. */
export function readFile(path: string): Promise<Uint8Array> {
  const parts: Uint8Array[] = []
  return new Promise((resolve, reject) => {
    remoteStorage()
      .read({ request: { path, offset: 0n } })
      .subscribe({
        next: (chunk) => parts.push(fromHex(chunk.bytes)),
        error: (error) => reject(error.reason ? new RemoteStorageError('read', error.reason) : error),
        complete: () => resolve(concat(parts)),
      })
  })
}

/** Store `content` at `path`, replacing any file already there. */
export async function writeFile(path: string, content: Blob | Uint8Array, contentType: string): Promise<void> {
  try {
    await storeFile(remoteStorage(), path, content, { contentType })
  } catch (error) {
    throw isCallError(error) ? new RemoteStorageError('store', error) : error
  }
}

/** Remove the file at `path`. */
export async function removeFile(path: string): Promise<void> {
  const result = await remoteStorage().remove({ path })
  if (result.isErr()) throw new RemoteStorageError('remove', result.error)
}

/**
 * The library's plan: how much it may hold, how much it holds, and until when.
 * `null` if the library has no plan.
 */
export async function getPlan(): Promise<RemoteStoragePlan | null> {
  const result = await remoteStorage().getPlan()
  if (result.isErr()) throw new RemoteStorageError('getPlan', result.error)
  return result.value ?? null
}

/**
 * What a plan of `bytes` funded until `fundedUntil` (seconds since the Unix
 * epoch) costs, without asking the user or paying. Fails with `NotAvailable`
 * if no backend offers it for at most `maxPrice`.
 */
export async function quotePlan(bytes: bigint, fundedUntil: bigint, maxPrice: bigint): Promise<RemoteStorageQuote> {
  const result = await remoteStorage().quotePlan({ bytes, fundedUntil, maxPrice })
  if (result.isErr()) throw new RemoteStorageError('quotePlan', result.error)
  return result.value
}

/**
 * Ask the user to pay for a plan of `bytes` funded until `fundedUntil`
 * (seconds since the Unix epoch), for at most `maxPrice`. Returns the plan
 * and what it cost.
 */
export async function setPlan(
  bytes: bigint,
  fundedUntil: bigint,
  maxPrice: bigint,
): Promise<{ plan: RemoteStoragePlan; price: bigint }> {
  const result = await remoteStorage().setPlan({ bytes, fundedUntil, maxPrice })
  if (result.isErr()) throw new RemoteStorageError('setPlan', result.error)
  return result.value
}

function remoteStorage() {
  const client = getClientSync()
  if (!client) throw new Error('Photos keeps its library in a TrUAPI host; open it in one.')
  return client.remoteStorage
}

function isCallError(error: unknown): error is CallErrorValue<unknown> {
  return typeof error === 'object' && error !== null && 'tag' in error
}

/** The error in a versioned domain error, `{ tag: "V1", value: { tag: "NotFound" } }`. */
function domainError(versioned: unknown): { tag: string; value?: unknown } | undefined {
  const error = (versioned as { value?: { tag?: unknown; value?: unknown } } | undefined)?.value
  return typeof error?.tag === 'string' ? { tag: error.tag, value: error.value } : undefined
}

function describe(error: CallErrorValue<unknown>): string {
  return JSON.stringify(error, (_key, value: unknown) => (typeof value === 'bigint' ? value.toString() : value))
}

function concat(parts: Uint8Array[]): Uint8Array {
  const joined = new Uint8Array(parts.reduce((total, part) => total + part.length, 0))
  let offset = 0
  for (const part of parts) {
    joined.set(part, offset)
    offset += part.length
  }
  return joined
}
