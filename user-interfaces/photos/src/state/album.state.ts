// SPDX-License-Identifier: GPL-3.0-only
//
// Album State — the M6 "State B" interaction layer. Once a library exists, this
// drives the provider's `/fs` API (the browser port of the M2/M3 headless flow):
// list/create albums (directories), upload photos with a client-generated
// thumbnail, render a grid from those thumbnails, and open a photo full-res. Every
// mutation ends by recomputing the drive's metadata Merkle root locally and
// anchoring it on-chain via `setRoot` (copy-on-write; the root moves each time).
//
// Mirrors `library.state.ts` conventions: raw `BehaviorSubject`s, `bind` hooks,
// and action functions that `.next(...)` them. Path layout matches
// `scripts/photos-flow.ts`: album `/<Album>`, photo `/<Album>/<file>`, thumb
// `/.thumbs/<Album>/<file>`.

import { BehaviorSubject } from 'rxjs'
import { bind } from '@react-rxjs/core'
import type { InjectedPolkadotAccount } from 'polkadot-api/pjs-signer'
import { fromHex, type ParachainApi } from '@web3-storage/papi'
import { getApi } from '@/lib/chain-client'
import type { ResolvedContract } from '@/lib/photos-contract'
import {
  downloadFile,
  listDir,
  mkdir,
  putFile,
  recomputeRoot,
  resolveFsContext,
  type CachedDataRoot,
  type FsContext,
} from '@/lib/fs-client'
import { computeDataRoot } from '@/lib/merkle'
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

const fsContext$ = new BehaviorSubject<FsContext | null>(null)
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

export const [useFsContext] = bind(fsContext$, null)
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

/** Per-file content root cache, seeded by `putFile`, so a re-anchor needn't re-download. */
let dataRootCache = new Map<string, CachedDataRoot>()
/** Live grid thumbnail object URLs, revoked when the grid reloads or the library resets. */
let gridUrls: string[] = []
/** Called after a successful `setRoot` so the page can re-read the on-chain anchor. */
let anchoredCallback: (() => void) | null = null

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
  onAnchored: () => void,
): Promise<void> {
  api = getApi()
  signer = account.polkadotSigner
  contractBytes = fromHex(contract.address)
  anchoredCallback = onAnchored

  if (currentDriveId === driveId && fsContext$.getValue()) {
    // Same drive — keep caches/selection, just refresh listings.
    await loadAlbums()
    return
  }

  resetSession()
  currentDriveId = driveId
  libraryError$.next(undefined)
  try {
    const ctx = await resolveFsContext(api, driveId)
    fsContext$.next(ctx)
    await loadAlbums()
  } catch (err) {
    libraryError$.next(err instanceof Error ? err.message : 'Could not reach the storage provider.')
  }
}

/** List top-level directories as albums (hiding the `.thumbs` subtree and dotfolders). */
export async function loadAlbums(): Promise<void> {
  const ctx = fsContext$.getValue()
  if (!ctx) return
  try {
    const listing = await listDir(ctx, '/')
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
  const ctx = fsContext$.getValue()
  const album = selectedAlbum$.getValue()
  if (!ctx || !album) {
    replaceGrid([])
    return
  }

  gridLoading$.next(true)
  try {
    const files = (await listDir(ctx, `/${album}`)).filter((e) => e.entryType === 'file')
    const items: GridItem[] = await Promise.all(
      files.map(async (f): Promise<GridItem> => {
        const thumbPath = `${THUMBS_ROOT}/${album}/${f.name}`
        let thumbUrl: string | undefined
        try {
          const bytes = await downloadFile(ctx, thumbPath)
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
 * then recompute + anchor the metadata root. Rejects an empty/slashed/dot name.
 */
export async function createAlbum(rawName: string): Promise<void> {
  const ctx = fsContext$.getValue()
  if (!ctx) return
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
    await ensureDir(ctx, `/${name}`)
    await ensureDir(ctx, THUMBS_ROOT)
    await ensureDir(ctx, `${THUMBS_ROOT}/${name}`)
    await reanchor()
    await loadAlbums()
    await selectAlbum(name)
  } catch (err) {
    libraryError$.next(err instanceof Error ? err.message : 'Could not create the album.')
  }
}

/**
 * Upload `files` into the selected album: for each, generate a thumbnail, PUT the
 * full photo and the thumbnail (verifying each against its local `data_root`),
 * then recompute + anchor the metadata root once for the whole batch.
 */
export async function uploadPhotos(files: File[]): Promise<void> {
  const ctx = fsContext$.getValue()
  const album = selectedAlbum$.getValue()
  if (!ctx || !album || files.length === 0) return

  libraryError$.next(undefined)
  uploads$.next({ total: files.length, done: 0 })
  try {
    // Make sure the thumbnail subtree for this album exists (older albums may predate it).
    await ensureDir(ctx, THUMBS_ROOT)
    await ensureDir(ctx, `${THUMBS_ROOT}/${album}`)

    for (let i = 0; i < files.length; i++) {
      const file = files[i]
      uploads$.next({ total: files.length, done: i, current: file.name })

      const bytes = new Uint8Array(await file.arrayBuffer())
      const photoPath = `/${album}/${file.name}`
      await putVerified(ctx, photoPath, bytes, file.type || 'application/octet-stream')

      try {
        const thumb = await makeThumbnail(file)
        await putVerified(ctx, `${THUMBS_ROOT}/${album}/${file.name}`, thumb.bytes, thumb.contentType)
      } catch {
        // Thumbnail generation failed (unsupported/corrupt image) — keep the full
        // photo; the grid will fall back for this one.
      }
    }
    uploads$.next({ total: files.length, done: files.length })

    await reanchor()
    await loadGrid()
  } catch (err) {
    libraryError$.next(err instanceof Error ? err.message : 'Upload failed.')
  } finally {
    uploads$.next(null)
  }
}

/** Open a photo full-resolution in the lightbox. */
export async function openPhoto(item: GridItem): Promise<void> {
  const ctx = fsContext$.getValue()
  if (!ctx) return
  lightboxLoading$.next(true)
  try {
    const bytes = await downloadFile(ctx, item.path)
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
 * the original blob lingers), regenerate its thumbnail, then recompute + anchor the
 * metadata root. Mirrors `uploadPhotos`: a thumbnail failure doesn't abort the edit,
 * and on error the editor stays open for retry.
 */
export async function saveEdit(editedBytes: Uint8Array, contentType: string): Promise<void> {
  const ctx = fsContext$.getValue()
  const photo = lightbox$.getValue()
  if (!ctx || !photo) return

  const { path, thumbPath } = photo.item
  libraryError$.next(undefined)
  try {
    await putVerified(ctx, path, editedBytes, contentType)

    try {
      const thumb = await makeThumbnail(new Blob([editedBytes as BlobPart], { type: contentType }))
      await putVerified(ctx, thumbPath, thumb.bytes, thumb.contentType)
    } catch {
      // Thumbnail regeneration failed — keep the edited full photo; the grid falls back.
    }

    await reanchor()
    closeEditor()
    closePhoto()
    await loadGrid()
  } catch (err) {
    libraryError$.next(err instanceof Error ? err.message : 'Could not save the edit.')
    throw err
  }
}

/** Tear down the album layer (on wallet/network/drive change). */
export function resetLibrary(): void {
  currentDriveId = null
  fsContext$.next(null)
  resetSession()
}

/** Dismiss a transient error banner. */
export function clearLibraryError(): void {
  libraryError$.next(undefined)
}

// ─────────────────────────────────────────────────────────────────────────────
// Internals
// ─────────────────────────────────────────────────────────────────────────────

/** Recompute the metadata root from the live tree and anchor it via `setRoot`. */
async function reanchor(): Promise<void> {
  const ctx = fsContext$.getValue()
  if (!ctx || !api || !signer || !contractBytes) return
  try {
    anchorStatus$.next({ stage: 'recomputing' })
    const root = await recomputeRoot(ctx, dataRootCache)
    anchorStatus$.next({ stage: 'anchoring' })
    await submitSetRoot(api, signer, contractBytes, rootToBytes32(root))
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

/** PUT a blob and assert the provider's `data_root` matches our local computation, caching it. */
async function putVerified(ctx: FsContext, path: string, bytes: Uint8Array, contentType: string): Promise<void> {
  const res = await putFile(ctx, path, bytes, contentType)
  const localRoot = computeDataRoot(bytes)
  if (rootToBytes32(localRoot).toLowerCase() !== res.dataRoot.toLowerCase()) {
    throw new Error(`Provider stored ${path} with a different content root than computed locally.`)
  }
  dataRootCache.set(path, { dataRoot: localRoot, size: BigInt(bytes.length) })
}

/** Create `path` if a directory of that name isn't already present in its parent. */
async function ensureDir(ctx: FsContext, path: string): Promise<void> {
  const slash = path.lastIndexOf('/')
  const parent = slash <= 0 ? '/' : path.slice(0, slash)
  const name = path.slice(slash + 1)
  const siblings = await listDir(ctx, parent)
  if (siblings.some((e) => e.entryType === 'directory' && e.name === name)) return
  await mkdir(ctx, path)
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
  dataRootCache = new Map()
  replaceGrid([])
  revokeLightbox()
  lightbox$.next(null)
  editorOpen$.next(false)
  albums$.next([])
  selectedAlbum$.next(null)
  uploads$.next(null)
  anchorStatus$.next({ stage: 'idle' })
}
