// SPDX-License-Identifier: GPL-3.0-only
//
// Album State — the M6 "State B" interaction layer. Once a library exists, this
// drives the provider's `/fs` API (the browser port of the M2/M3 headless flow):
// list/create albums (directories), upload photos with a client-generated
// thumbnail, render a grid from those thumbnails, and open a photo full-res. Every
// mutation refreshes its listing immediately and then schedules a background
// re-anchor (`scheduleReanchor`): a single-flight, coalescing worker recomputes the
// drive's metadata Merkle root locally and anchors it on-chain via `setRoot`
// (copy-on-write; the root moves each time) without blocking further interaction.
//
// Mirrors `library.state.ts` conventions: raw `BehaviorSubject`s, `bind` hooks,
// and action functions that `.next(...)` them. Path layout matches
// `scripts/photos-flow.ts`: album `/<Album>`, photo `/<Album>/<file>`, thumb
// `/.thumbs/<Album>/<file>`.

import { BehaviorSubject } from 'rxjs'
import { bind } from '@react-rxjs/core'
import type { InjectedPolkadotAccount } from 'polkadot-api/pjs-signer'
import { fromHex, type ParachainApi } from '@web3-storage/papi'
import { requireApi } from '@/lib/chain-client'
import type { ResolvedContract } from '@/lib/photos-contract'
import { getFsClient, resolveBucketId } from '@/lib/fs-client'
import { recomputeRoot } from '@/lib/fs-root'
import { computeDataRoot, toHex } from '@web3-storage/sdk'
import { LocalIndex } from '@/lib/local-index'
import { loadIndex, saveIndex } from '@/lib/index-store'
import { getParachainWs } from '@/state/network.state'
import { rootToBytes32, submitSetRoot } from '@/lib/photos-contract-write'
import { makeThumbnail } from '@/lib/thumbnail'

/** Parallel `.thumbs/` subtree that holds the downscaled grid thumbnails. */
const THUMBS_ROOT = '/.thumbs'

/** A photo as rendered in the grid (thumbnail-backed). */
export interface GridItem {
  name: string
  /** Full-resolution path, e.g. `/Beach/photo.jpg`. */
  path: string
  /** Thumbnail path, e.g. `/.thumbs/Beach/photo.jpg`. */
  thumbPath: string
  /** Object URL for the loaded thumbnail, or undefined while loading / on miss. */
  thumbUrl?: string
  /** Full-resolution byte size. */
  size: number
}

/** Batch upload progress. */
export interface UploadProgress {
  total: number
  done: number
  current?: string
}

export type AnchorStage = 'idle' | 'recomputing' | 'anchoring' | 'done' | 'error'

export interface AnchorStatus {
  stage: AnchorStage
  message?: string
}

/** The currently-open lightbox photo (full-resolution object URL). */
export interface LightboxState {
  name: string
  url: string
  /** The grid item it was opened from — carries the path/thumbPath the editor needs. */
  item: GridItem
}

// ─────────────────────────────────────────────────────────────────────────────
// State
// ─────────────────────────────────────────────────────────────────────────────

const bucketId$ = new BehaviorSubject<bigint | null>(null)
const libraryError$ = new BehaviorSubject<string | undefined>(undefined)
const albums$ = new BehaviorSubject<string[]>([])
const selectedAlbum$ = new BehaviorSubject<string | null>(null)
const entries$ = new BehaviorSubject<GridItem[]>([])
const gridLoading$ = new BehaviorSubject<boolean>(false)
const uploads$ = new BehaviorSubject<UploadProgress | null>(null)
const anchorStatus$ = new BehaviorSubject<AnchorStatus>({ stage: 'idle' })
const lightbox$ = new BehaviorSubject<LightboxState | null>(null)
const lightboxLoading$ = new BehaviorSubject<boolean>(false)
const editorOpen$ = new BehaviorSubject<boolean>(false)

export const [useBucketId] = bind(bucketId$, null)
export const [useLibraryError] = bind(libraryError$, undefined)
export const [useAlbums] = bind(albums$, [])
export const [useSelectedAlbum] = bind(selectedAlbum$, null)
export const [useEntries] = bind(entries$, [])
export const [useGridLoading] = bind(gridLoading$, false)
export const [useUploads] = bind(uploads$, null)
export const [useAnchorStatus] = bind(anchorStatus$, { stage: 'idle' })
export const [useLightbox] = bind(lightbox$, null)
export const [useLightboxLoading] = bind(lightboxLoading$, false)
export const [useEditorOpen] = bind(editorOpen$, false)

// ─────────────────────────────────────────────────────────────────────────────
// Non-reactive session state (current library + caches)
// ─────────────────────────────────────────────────────────────────────────────

let api: ParachainApi | null = null
let signer: InjectedPolkadotAccount['polkadotSigner'] | null = null
let contractBytes: Uint8Array | null = null
let currentDriveId: bigint | null = null

/**
 * Client-maintained drive index — the source of truth for the anchored metadata
 * root, updated on every upload/edit/delete. `indexAuthoritative` is true once the
 * index is known to fully describe the tree (a freshly-created drive, a persisted
 * snapshot that matched the on-chain anchor, or a completed provider recompute); it
 * then anchors from `index.root()` with no downloads. `indexKey` is the per-drive
 * IndexedDB key (`${parachainWs}:${driveId}`) the index persists under.
 */
let index = new LocalIndex()
let indexAuthoritative = false
let indexKey: string | null = null
/** Live grid thumbnail object URLs, revoked when the grid reloads or the library resets. */
let gridUrls: string[] = []
/** Called after a successful `setRoot` so the page can re-read the on-chain anchor. */
let anchoredCallback: (() => void) | null = null
/** Set by `scheduleReanchor` when the tree changed; drained by the background anchor worker. */
let anchorDirty = false
/** True while the single-flight background anchor worker is running. */
let anchorRunning = false

// ─────────────────────────────────────────────────────────────────────────────
// Actions
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Bind the album layer to a library: resolve its `/fs` context from chain state
 * and load its albums. Safe to call repeatedly; re-resolves only when the drive
 * changes. `onAnchored` is invoked after each successful `setRoot` so the page
 * can refresh the on-chain anchor it displays.
 */
export async function initLibrary(
  driveId: bigint,
  contract: ResolvedContract,
  account: InjectedPolkadotAccount,
  rootCid: `0x${string}`,
  onAnchored: () => void,
): Promise<void> {
  api = requireApi()
  signer = account.polkadotSigner
  contractBytes = fromHex(contract.address)
  anchoredCallback = onAnchored

  if (currentDriveId === driveId && bucketId$.getValue() !== null) {
    // Same drive — keep caches/selection, just refresh listings.
    await loadAlbums()
    return
  }

  resetSession()
  currentDriveId = driveId
  libraryError$.next(undefined)
  try {
    const bucketId = await resolveBucketId(driveId)
    bucketId$.next(bucketId)
    await loadPersistedIndex(driveId, rootCid)
    await loadAlbums()
  } catch (err) {
    libraryError$.next(err instanceof Error ? err.message : 'Could not reach the storage provider.')
  }
}

/**
 * Seed the in-memory index for `driveId` from its persisted snapshot, trusting it
 * only if its root still matches the on-chain anchor. A match ⇒ authoritative:
 * future roots anchor from `index.root()` with no downloads. A miss or mismatch
 * (cold cache, a freshly-created drive with no snapshot, or a drive mutated
 * elsewhere) ⇒ start empty and fall back to a provider recompute on the next
 * anchor, which repopulates the index and persists it for subsequent reloads.
 */
async function loadPersistedIndex(driveId: bigint, rootCid: `0x${string}`): Promise<void> {
  indexKey = `${getParachainWs()}:${driveId}`
  const persisted = await loadIndex(indexKey)
  if (persisted && toHex(persisted.root()).toLowerCase() === rootCid.toLowerCase()) {
    index = persisted
    indexAuthoritative = true
  } else {
    index = new LocalIndex()
    indexAuthoritative = false
  }
}

/** List top-level directories as albums (hiding the `.thumbs` subtree and dotfolders). */
export async function loadAlbums(): Promise<void> {
  const bucketId = bucketId$.getValue()
  if (bucketId === null) return
  try {
    const listing = await getFsClient().listDirectory(bucketId, '/')
    const names = listing
      .filter((e) => e.entryType === 'directory' && !e.name.startsWith('.'))
      .map((e) => e.name)
      .sort()
    albums$.next(names)

    // Keep the current selection if still present; otherwise pick the first album.
    const selected = selectedAlbum$.getValue()
    if (selected && names.includes(selected)) {
      await loadGrid()
    } else if (names.length > 0) {
      await selectAlbum(names[0])
    } else {
      selectedAlbum$.next(null)
      replaceGrid([])
    }
  } catch (err) {
    libraryError$.next(err instanceof Error ? err.message : 'Could not list albums.')
  }
}

/** Select an album and (re)load its photo grid. */
export async function selectAlbum(name: string): Promise<void> {
  selectedAlbum$.next(name)
  await loadGrid()
}

/** List the selected album's files and load each photo's thumbnail into the grid. */
export async function loadGrid(): Promise<void> {
  const bucketId = bucketId$.getValue()
  const album = selectedAlbum$.getValue()
  if (bucketId === null || !album) {
    replaceGrid([])
    return
  }

  gridLoading$.next(true)
  try {
    const files = (await getFsClient().listDirectory(bucketId, `/${album}`)).filter((e) => e.entryType === 'file')
    const items: GridItem[] = await Promise.all(
      files.map(async (f): Promise<GridItem> => {
        const thumbPath = `${THUMBS_ROOT}/${album}/${f.name}`
        let thumbUrl: string | undefined
        try {
          const bytes = await getFsClient().downloadFile(bucketId, thumbPath)
          thumbUrl = URL.createObjectURL(new Blob([bytes as BlobPart], { type: 'image/jpeg' }))
        } catch {
          // No thumbnail (e.g. uploaded outside the app) — the grid shows a fallback.
        }
        return { name: f.name, path: `/${album}/${f.name}`, thumbPath, thumbUrl, size: f.size }
      }),
    )
    replaceGrid(items)
  } catch (err) {
    libraryError$.next(err instanceof Error ? err.message : 'Could not load photos.')
    replaceGrid([])
  } finally {
    gridLoading$.next(false)
  }
}

/**
 * Create a new album: make `/<name>` plus its parallel `/.thumbs/<name>` subtree,
 * then schedule a background re-anchor. Rejects an empty/slashed/dot name.
 */
export async function createAlbum(rawName: string): Promise<void> {
  const bucketId = bucketId$.getValue()
  if (bucketId === null) return
  const name = rawName.trim()
  if (!name || name.includes('/') || name.startsWith('.')) {
    libraryError$.next('Album name cannot be empty, contain "/", or start with ".".')
    return
  }
  if (albums$.getValue().includes(name)) {
    libraryError$.next(`An album named "${name}" already exists.`)
    return
  }

  libraryError$.next(undefined)
  try {
    await ensureDir(bucketId, `/${name}`)
    await ensureDir(bucketId, THUMBS_ROOT)
    await ensureDir(bucketId, `${THUMBS_ROOT}/${name}`)
    await loadAlbums()
    await selectAlbum(name)
    scheduleReanchor()
  } catch (err) {
    libraryError$.next(err instanceof Error ? err.message : 'Could not create the album.')
  }
}

/**
 * Upload `files` into the selected album: for each, generate a thumbnail, PUT the
 * full photo and the thumbnail (verifying each against its local `data_root`),
 * refresh the grid, then schedule a background re-anchor for the whole batch.
 */
export async function uploadPhotos(files: File[]): Promise<void> {
  const bucketId = bucketId$.getValue()
  const album = selectedAlbum$.getValue()
  if (bucketId === null || !album || files.length === 0) return

  libraryError$.next(undefined)
  uploads$.next({ total: files.length, done: 0 })
  try {
    // Make sure the thumbnail subtree for this album exists (older albums may predate it).
    await ensureDir(bucketId, THUMBS_ROOT)
    await ensureDir(bucketId, `${THUMBS_ROOT}/${album}`)

    for (let i = 0; i < files.length; i++) {
      const file = files[i]
      uploads$.next({ total: files.length, done: i, current: file.name })

      const bytes = new Uint8Array(await file.arrayBuffer())
      const photoPath = `/${album}/${file.name}`
      await putVerified(bucketId, photoPath, bytes, file.type || 'application/octet-stream')

      try {
        const thumb = await makeThumbnail(file)
        await putVerified(bucketId, `${THUMBS_ROOT}/${album}/${file.name}`, thumb.bytes, thumb.contentType)
      } catch {
        // Thumbnail generation failed (unsupported/corrupt image) — keep the full
        // photo; the grid will fall back for this one.
      }
    }
    uploads$.next({ total: files.length, done: files.length })

    await loadGrid()
    scheduleReanchor()
  } catch (err) {
    libraryError$.next(err instanceof Error ? err.message : 'Upload failed.')
  } finally {
    uploads$.next(null)
  }
}

/** Open a photo full-resolution in the lightbox. */
export async function openPhoto(item: GridItem): Promise<void> {
  const bucketId = bucketId$.getValue()
  if (bucketId === null) return
  lightboxLoading$.next(true)
  try {
    const bytes = await getFsClient().downloadFile(bucketId, item.path)
    revokeLightbox()
    const url = URL.createObjectURL(new Blob([bytes as BlobPart]))
    lightbox$.next({ name: item.name, url, item })
  } catch (err) {
    libraryError$.next(err instanceof Error ? err.message : 'Could not open the photo.')
  } finally {
    lightboxLoading$.next(false)
  }
}

/** Close the lightbox and release its object URL (also closes the editor). */
export function closePhoto(): void {
  editorOpen$.next(false)
  revokeLightbox()
  lightbox$.next(null)
}

/** Open the crop/rotate editor for the photo currently in the lightbox. */
export function openEditor(): void {
  if (lightbox$.getValue()) editorOpen$.next(true)
}

/** Close the editor, returning to the lightbox. */
export function closeEditor(): void {
  editorOpen$.next(false)
}

/**
 * Save an edited photo (M7). Re-PUT `editedBytes` to the open photo's *same* path
 * (copy-on-write: a new content-addressed blob is written and the path repointed;
 * the original blob lingers), regenerate its thumbnail, refresh the grid, then
 * schedule a background re-anchor. Mirrors `uploadPhotos`: a thumbnail failure
 * doesn't abort the edit, and on a PUT error the editor stays open for retry.
 */
export async function saveEdit(editedBytes: Uint8Array, contentType: string): Promise<void> {
  const bucketId = bucketId$.getValue()
  const photo = lightbox$.getValue()
  if (bucketId === null || !photo) return

  const { path, thumbPath } = photo.item
  libraryError$.next(undefined)
  try {
    await putVerified(bucketId, path, editedBytes, contentType)

    try {
      const thumb = await makeThumbnail(new Blob([editedBytes as BlobPart], { type: contentType }))
      await putVerified(bucketId, thumbPath, thumb.bytes, thumb.contentType)
    } catch {
      // Thumbnail regeneration failed — keep the edited full photo; the grid falls back.
    }

    closeEditor()
    closePhoto()
    await loadGrid()
    scheduleReanchor()
  } catch (err) {
    libraryError$.next(err instanceof Error ? err.message : 'Could not save the edit.')
    throw err
  }
}

/**
 * Rename a photo within its album, keeping it content-addressed. The provider has
 * no move op, so this copies the bytes to the new path (photo + thumbnail) and
 * deletes the old one — re-PUTting identical bytes yields the same `data_root`, so
 * no content is duplicated. Refreshes the grid and schedules a background re-anchor.
 * `rawName` is the full new filename (the caller preserves the extension).
 */
export async function renamePhoto(item: GridItem, rawName: string): Promise<void> {
  const bucketId = bucketId$.getValue()
  const album = selectedAlbum$.getValue()
  if (bucketId === null || !album) return

  const name = rawName.trim()
  if (!name || name.includes('/') || name.startsWith('.')) {
    libraryError$.next('Name cannot be empty, contain "/", or start with ".".')
    return
  }
  if (name === item.name) return
  if (entries$.getValue().some((e) => e.name === name)) {
    libraryError$.next(`A photo named "${name}" already exists in this album.`)
    return
  }

  const newPath = `/${album}/${name}`
  const newThumbPath = `${THUMBS_ROOT}/${album}/${name}`
  libraryError$.next(undefined)
  try {
    // Copy first; only delete the old path once the new one is safely in place.
    const photo = await getFsClient().downloadFileWithType(bucketId, item.path)
    await putVerified(bucketId, newPath, photo.bytes, photo.contentType)
    try {
      const thumb = await getFsClient().downloadFileWithType(bucketId, item.thumbPath)
      await putVerified(bucketId, newThumbPath, thumb.bytes, thumb.contentType)
    } catch {
      // No thumbnail to carry over (e.g. uploaded outside the app) — skip it.
    }

    await getFsClient().deleteFile(bucketId, item.path)
    await getFsClient().deleteFile(bucketId, item.thumbPath).catch(() => {})
    index.remove(item.path)
    index.remove(item.thumbPath)

    await loadGrid()
    scheduleReanchor()
  } catch (err) {
    libraryError$.next(err instanceof Error ? err.message : 'Could not rename the photo.')
  }
}

/**
 * Delete a photo (and its thumbnail) from the album. Removes the FS index entries;
 * the underlying blobs linger in the MMR (no GC), matching the app's copy-on-write
 * model. Refreshes the grid and schedules a background re-anchor.
 */
export async function deletePhoto(item: GridItem): Promise<void> {
  const bucketId = bucketId$.getValue()
  if (bucketId === null) return

  libraryError$.next(undefined)
  try {
    await getFsClient().deleteFile(bucketId, item.path)
    await getFsClient().deleteFile(bucketId, item.thumbPath).catch(() => {})
    index.remove(item.path)
    index.remove(item.thumbPath)

    await loadGrid()
    scheduleReanchor()
  } catch (err) {
    libraryError$.next(err instanceof Error ? err.message : 'Could not delete the photo.')
  }
}

/** Tear down the album layer (on wallet/network/drive change). */
export function resetLibrary(): void {
  currentDriveId = null
  bucketId$.next(null)
  resetSession()
}

/** Dismiss a transient error banner. */
export function clearLibraryError(): void {
  libraryError$.next(undefined)
}

// ─────────────────────────────────────────────────────────────────────────────
// Internals
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Mark the metadata root dirty and ensure the background anchor worker is running.
 * Mutations call this instead of awaiting the anchor, so the user can keep working
 * while the (slow) recompute + `setRoot` tx happen in the background.
 */
function scheduleReanchor(): void {
  anchorDirty = true
  void runAnchorWorker()
}

/**
 * Single-flight background worker that anchors the latest tree state. Coalesces
 * concurrent mutations: each `scheduleReanchor` sets `anchorDirty`, and the worker
 * keeps re-running until nothing is pending, so only one `setRoot` tx is ever in
 * flight and the on-chain anchor converges to the live tree. On failure it stops
 * (rather than hot-looping); since the next mutation reschedules and `recomputeRoot`
 * always covers the whole tree, no change is lost once anchoring succeeds again.
 */
async function runAnchorWorker(): Promise<void> {
  if (anchorRunning) return
  anchorRunning = true
  try {
    while (anchorDirty) {
      anchorDirty = false
      await runReanchor()
    }
  } catch {
    // `runReanchor` already surfaced the failure via `anchorStatus$`.
  } finally {
    anchorRunning = false
  }
}

/** Recompute the metadata root from the live tree and anchor it via `setRoot`. */
async function runReanchor(): Promise<void> {
  // Snapshot the session up front: a background anchor can outlive a library/account
  // switch that reassigns these module-level vars, and it must sign for the drive it
  // started on (not whatever is selected by the time the slow recompute finishes).
  const bucketId = bucketId$.getValue()
  const sessionApi = api
  const sessionSigner = signer
  const sessionContract = contractBytes
  const sessionIndex = index
  const sessionKey = indexKey
  const sessionAuthoritative = indexAuthoritative
  if (bucketId === null || !sessionApi || !sessionSigner || !sessionContract) return
  try {
    anchorStatus$.next({ stage: 'recomputing' })
    // Authoritative ⇒ the index already mirrors the whole tree, so anchor its root
    // directly (no provider round-trip). Otherwise recompute from the provider,
    // which repopulates the index — mark it authoritative for the next anchor
    // (unless a library switch swapped the module index out from under us).
    let root: Uint8Array
    if (sessionAuthoritative) {
      root = sessionIndex.root()
    } else {
      root = await recomputeRoot(getFsClient(), bucketId, sessionIndex)
      if (index === sessionIndex) indexAuthoritative = true
    }
    anchorStatus$.next({ stage: 'anchoring' })
    await submitSetRoot(sessionApi, sessionSigner, sessionContract, rootToBytes32(root))
    // Snapshot the now-anchored index so a reload matches the on-chain root and
    // skips the recompute. Never throws (see `saveIndex`).
    if (sessionKey) await saveIndex(sessionKey, sessionIndex)
    anchorStatus$.next({ stage: 'done' })
    anchoredCallback?.()
  } catch (err) {
    anchorStatus$.next({
      stage: 'error',
      message: err instanceof Error ? err.message : 'Could not anchor the library root on-chain.',
    })
    throw err
  }
}

/** PUT a blob, assert the provider's `data_root` matches our local computation, and record it in the index. */
async function putVerified(bucketId: bigint, path: string, bytes: Uint8Array, contentType: string): Promise<void> {
  const res = await getFsClient().uploadFile(bucketId, path, bytes, { contentType })
  const localRoot = computeDataRoot(bytes)
  if (!res.dataRoot || rootToBytes32(localRoot).toLowerCase() !== res.dataRoot.toLowerCase()) {
    throw new Error(`Provider stored ${path} with a different content root than computed locally.`)
  }
  index.setFile(path, localRoot, BigInt(bytes.length))
}

/** Create `path` if a directory of that name isn't already present in its parent; record it in the index. */
async function ensureDir(bucketId: bigint, path: string): Promise<void> {
  const slash = path.lastIndexOf('/')
  const parent = slash <= 0 ? '/' : path.slice(0, slash)
  const name = path.slice(slash + 1)
  const client = getFsClient()
  const siblings = await client.listDirectory(bucketId, parent)
  if (!siblings.some((e) => e.entryType === 'directory' && e.name === name)) await client.createDirectory(bucketId, path)
  index.setDir(path)
}

/** Replace the grid, revoking the previous batch's thumbnail object URLs. */
function replaceGrid(items: GridItem[]): void {
  for (const url of gridUrls) URL.revokeObjectURL(url)
  gridUrls = items.map((i) => i.thumbUrl).filter((u): u is string => !!u)
  entries$.next(items)
}

function revokeLightbox(): void {
  const current = lightbox$.getValue()
  if (current) URL.revokeObjectURL(current.url)
}

/** Clear per-library caches, grid, selection, and any open lightbox. */
function resetSession(): void {
  index = new LocalIndex()
  indexAuthoritative = false
  indexKey = null
  replaceGrid([])
  revokeLightbox()
  lightbox$.next(null)
  editorOpen$.next(false)
  albums$.next([])
  selectedAlbum$.next(null)
  uploads$.next(null)
  anchorStatus$.next({ stage: 'idle' })
  // Stop the background anchor worker from coalescing across a library/account switch.
  anchorDirty = false
}
