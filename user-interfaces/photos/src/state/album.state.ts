// SPDX-License-Identifier: GPL-3.0-only
//
// Album State — the library's interaction layer over the host's remote storage
// (`lib/remote-library.ts`): list albums, upload photos with a client-generated
// thumbnail, render a grid from those thumbnails, open a photo full-res, and
// rename, edit or delete one. The host keeps the file tree in the library's
// bucket, so a second device sees the same albums.
//
// Path layout: album `/<Album>`, photo `/<Album>/<file>`, thumbnail
// `/.thumbs/<Album>/<file>`. Remote storage has files only, so an album is the
// top-level directory of its files, and a new album holds an empty
// `/<Album>/.album` marker until its first photo.

import { BehaviorSubject } from 'rxjs'
import { bind } from '@react-rxjs/core'
import {
  listFiles,
  readFile,
  removeFile,
  writeFile,
  RemoteStorageError,
  type LibraryFile,
} from '@/lib/remote-library'
import { makeThumbnail } from '@/lib/thumbnail'
import { loadPlan } from '@/state/library.state'

/** Parallel `.thumbs/` subtree that holds the downscaled grid thumbnails. */
const THUMBS_ROOT = '/.thumbs'

/** The empty file that keeps an album with no photos. */
const ALBUM_MARKER = '.album'

/** The album that receives uploads made from the all-photos view. */
export const DEFAULT_ALBUM = 'Camera roll'

/** A photo as rendered in the grid (thumbnail-backed). */
export interface GridItem {
  name: string
  /** The album the photo is in. */
  album: string
  /** Full-resolution path, e.g. `/Beach/photo.jpg`. */
  path: string
  /** Thumbnail path, e.g. `/.thumbs/Beach/photo.jpg`. */
  thumbPath: string
  /** Object URL for the loaded thumbnail, or undefined while loading / on miss. */
  thumbUrl?: string
  /** Full-resolution byte size. */
  size: number
  /** Last stored, in milliseconds since the Unix epoch. */
  modified: number
}

/** Batch upload progress. */
export interface UploadProgress {
  total: number
  done: number
  current?: string
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

const libraryError$ = new BehaviorSubject<string | undefined>(undefined)
const albums$ = new BehaviorSubject<string[]>([])
/** The album shown in the grid, or `null` for every album's photos. */
const selectedAlbum$ = new BehaviorSubject<string | null>(null)
const entries$ = new BehaviorSubject<GridItem[]>([])
const gridLoading$ = new BehaviorSubject<boolean>(false)
const uploads$ = new BehaviorSubject<UploadProgress | null>(null)
const lightbox$ = new BehaviorSubject<LightboxState | null>(null)
const lightboxLoading$ = new BehaviorSubject<boolean>(false)
const editorOpen$ = new BehaviorSubject<boolean>(false)
/** A photo the user deleted that stays restorable until `UNDO_WINDOW_MS` passes. */
const pendingDelete$ = new BehaviorSubject<GridItem | null>(null)

export const [useLibraryError] = bind(libraryError$, undefined)
export const [useAlbums] = bind(albums$, [])
export const [useSelectedAlbum] = bind(selectedAlbum$, null)
export const [useEntries] = bind(entries$, [])
export const [useGridLoading] = bind(gridLoading$, false)
export const [useUploads] = bind(uploads$, null)
export const [useLightbox] = bind(lightbox$, null)
export const [useLightboxLoading] = bind(lightboxLoading$, false)
export const [useEditorOpen] = bind(editorOpen$, false)
export const [usePendingDelete] = bind(pendingDelete$, null)

/** How long a deleted photo stays restorable before it is removed. */
export const UNDO_WINDOW_MS = 8_000

// ─────────────────────────────────────────────────────────────────────────────
// Non-reactive session state
// ─────────────────────────────────────────────────────────────────────────────

/** The library's files as last listed. */
let files: LibraryFile[] = []
/**
 * Live grid thumbnail object URLs keyed by thumb path. Reused across grid reloads
 * so a single-photo mutation doesn't re-download the whole album; an entry is
 * revoked when its path leaves the grid (`replaceGrid`), when its content is
 * rewritten (`store`), or when the library resets.
 */
const gridThumbUrls = new Map<string, string>()
/**
 * The tail of the write queue. The host commits each write on top of the root it
 * read, and retries when another write committed first, so concurrent writes
 * from this device would only race each other. Writes run one at a time.
 */
let writes: Promise<unknown> = Promise.resolve()
/** Removes the pending delete's photo when the undo window ends. */
let pendingDeleteTimer: ReturnType<typeof setTimeout> | undefined

// ─────────────────────────────────────────────────────────────────────────────
// Actions
// ─────────────────────────────────────────────────────────────────────────────

/** Load the library's albums. Safe to call repeatedly. */
export async function initLibrary(): Promise<void> {
  libraryError$.next(undefined)
  await loadAlbums()
}

/** List the albums (hiding the `.thumbs` subtree and dotfolders) and refresh the grid. */
export async function loadAlbums(): Promise<void> {
  try {
    files = await listFiles()
    const names = albumsOf(files)
    albums$.next(names)

    // Keep the current album if it still exists; otherwise show every photo.
    const selected = selectedAlbum$.getValue()
    if (selected && !names.includes(selected)) selectedAlbum$.next(null)
    await loadGrid()
  } catch (err) {
    libraryError$.next(message(err, 'Could not list albums.'))
  }
}

/** Select an album, or `null` for every photo, and (re)load the grid. */
export async function selectAlbum(name: string | null): Promise<void> {
  selectedAlbum$.next(name)
  await loadGrid()
}

/**
 * Show the selected album's photos, or every album's photos, newest first, and
 * load each one's thumbnail.
 */
export async function loadGrid(): Promise<void> {
  const selected = selectedAlbum$.getValue()

  gridLoading$.next(true)
  try {
    const paths = new Set(files.map((f) => f.path))
    const albums = new Set(albumsOf(files))
    const photos = files
      .filter((f) => {
        const album = albumOf(f.path)
        return (
          album !== null &&
          albums.has(album) &&
          (selected === null || album === selected) &&
          isPhoto(f.path) &&
          f.path !== pendingDelete$.getValue()?.path
        )
      })
      .sort((a, b) => b.modified - a.modified)
    const items: GridItem[] = await Promise.all(
      photos.map(async (f): Promise<GridItem> => {
        const album = albumOf(f.path) as string
        const name = baseName(f.path)
        const thumbPath = `${THUMBS_ROOT}/${album}/${name}`
        // Reuse an already-loaded thumbnail; only download ones we haven't cached
        // (new/changed paths — `store` evicts a thumb path when rewritten).
        let thumbUrl = gridThumbUrls.get(thumbPath)
        if (!thumbUrl && paths.has(thumbPath)) {
          try {
            const bytes = await readFile(thumbPath)
            thumbUrl = URL.createObjectURL(new Blob([bytes as BlobPart], { type: 'image/jpeg' }))
            gridThumbUrls.set(thumbPath, thumbUrl)
          } catch {
            // Unreadable thumbnail — the grid shows a fallback.
          }
        }
        return { name, album, path: f.path, thumbPath, thumbUrl, size: f.size, modified: f.modified }
      }),
    )
    replaceGrid(items)
  } catch (err) {
    libraryError$.next(message(err, 'Could not load photos.'))
    replaceGrid([])
  } finally {
    gridLoading$.next(false)
  }
}

/** Create a new album, kept by its marker file until its first photo. */
export async function createAlbum(rawName: string): Promise<void> {
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
    await store(`/${name}/${ALBUM_MARKER}`, new Uint8Array(), 'application/octet-stream')
    await loadAlbums()
    await selectAlbum(name)
  } catch (err) {
    libraryError$.next(message(err, 'Could not create the album.'))
  }
}

/**
 * Upload `files` into the selected album, or into `DEFAULT_ALBUM` from the
 * all-photos view, each with a generated thumbnail.
 */
export async function uploadPhotos(picked: File[]): Promise<void> {
  const album = selectedAlbum$.getValue() ?? DEFAULT_ALBUM
  if (picked.length === 0) return

  libraryError$.next(undefined)
  uploads$.next({ total: picked.length, done: 0 })
  let uploaded = 0
  try {
    for (const [done, file] of picked.entries()) {
      uploads$.next({ total: picked.length, done, current: file.name })
      await store(`/${album}/${file.name}`, file, file.type || 'application/octet-stream')
      uploaded++
      try {
        const thumb = await makeThumbnail(file)
        await store(`${THUMBS_ROOT}/${album}/${file.name}`, thumb.bytes, thumb.contentType)
      } catch {
        // Thumbnail generation failed (unsupported/corrupt image) — keep the full
        // photo; the grid will fall back for this one.
      }
    }
  } catch (err) {
    libraryError$.next(message(err, 'Upload failed.'))
  } finally {
    // Show whatever made it in, even if the batch stopped partway.
    if (uploaded > 0) await loadAlbums()
    uploads$.next(null)
  }
}

/** Open a photo full-resolution in the lightbox. */
export async function openPhoto(item: GridItem): Promise<void> {
  lightboxLoading$.next(true)
  try {
    const bytes = await readFile(item.path)
    revokeLightbox()
    const url = URL.createObjectURL(new Blob([bytes as BlobPart]))
    lightbox$.next({ name: item.name, url, item })
  } catch (err) {
    libraryError$.next(message(err, 'Could not open the photo.'))
  } finally {
    lightboxLoading$.next(false)
  }
}

/**
 * The photo `offset` places after the open one in the grid (negative for
 * earlier ones), or `undefined` past either end.
 */
export function adjacentPhoto(offset: number): GridItem | undefined {
  const current = lightbox$.getValue()
  if (!current) return undefined
  const entries = entries$.getValue()
  const index = entries.findIndex((e) => e.path === current.item.path)
  return index === -1 ? undefined : entries[index + offset]
}

/** Open the photo `offset` places after the open one. Does nothing past either end. */
export async function openAdjacentPhoto(offset: number): Promise<void> {
  if (lightboxLoading$.getValue()) return
  const next = adjacentPhoto(offset)
  if (next) await openPhoto(next)
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
 * Save an edited photo to the open photo's same path, regenerate its thumbnail,
 * and refresh the grid. A thumbnail failure doesn't abort the edit, and on a
 * store error the editor stays open for retry.
 */
export async function saveEdit(editedBytes: Uint8Array, contentType: string): Promise<void> {
  const photo = lightbox$.getValue()
  if (!photo) return

  const { path, thumbPath } = photo.item
  libraryError$.next(undefined)
  try {
    await store(path, editedBytes, contentType)

    try {
      const thumb = await makeThumbnail(new Blob([editedBytes as BlobPart], { type: contentType }))
      await store(thumbPath, thumb.bytes, thumb.contentType)
    } catch {
      // Thumbnail regeneration failed — keep the edited full photo; the grid falls back.
    }

    closeEditor()
    closePhoto()
    await loadAlbums()
  } catch (err) {
    libraryError$.next(message(err, 'Could not save the edit.'))
    throw err
  }
}

/**
 * Rename a photo and its thumbnail within the album. Remote storage has no
 * rename, so each is stored at its new path and removed from the old one.
 * `rawName` is the full new filename (the caller preserves the extension).
 */
export async function renamePhoto(item: GridItem, rawName: string): Promise<void> {
  const { album } = item
  const name = rawName.trim()
  if (!name || name.includes('/') || name.startsWith('.')) {
    libraryError$.next('Name cannot be empty, contain "/", or start with ".".')
    return
  }
  if (name === item.name) return
  if (files.some((f) => f.path === `/${album}/${name}`)) {
    libraryError$.next(`A photo named "${name}" already exists in this album.`)
    return
  }

  libraryError$.next(undefined)
  const path = `/${album}/${name}`
  const thumbPath = `${THUMBS_ROOT}/${album}/${name}`
  try {
    await move(item.path, path)
    try {
      await move(item.thumbPath, thumbPath)
    } catch (err) {
      // No thumbnail to carry over (e.g. uploaded outside the app) — skip it.
      if (!isNotFound(err)) throw err
    }
    // Keep the lightbox on the renamed photo.
    const open = lightbox$.getValue()
    if (open?.item.path === item.path) lightbox$.next({ ...open, name, item: { ...open.item, name, path, thumbPath } })
    await loadAlbums()
  } catch (err) {
    libraryError$.next(message(err, 'Could not rename the photo.'))
  }
}

/**
 * Hide a photo from the grid now and remove it when `UNDO_WINDOW_MS` passes,
 * unless `undoDelete` runs first. An earlier pending delete is removed at once.
 */
export function deleteWithUndo(item: GridItem): void {
  void commitPendingDelete()
  pendingDelete$.next(item)
  entries$.next(entries$.getValue().filter((e) => e.path !== item.path))
  pendingDeleteTimer = setTimeout(() => void commitPendingDelete(), UNDO_WINDOW_MS)
}

/** Restore the pending delete's photo to the grid. */
export async function undoDelete(): Promise<void> {
  clearTimeout(pendingDeleteTimer)
  if (!pendingDelete$.getValue()) return
  pendingDelete$.next(null)
  await loadGrid()
}

/** Remove the pending delete's photo now. */
export async function commitPendingDelete(): Promise<void> {
  clearTimeout(pendingDeleteTimer)
  const item = pendingDelete$.getValue()
  if (!item) return
  pendingDelete$.next(null)
  await deletePhoto(item)
}

/** Delete a photo and its thumbnail from the album. */
async function deletePhoto(item: GridItem): Promise<void> {
  libraryError$.next(undefined)
  try {
    await remove(item.path)
    await remove(item.thumbPath).catch((err: unknown) => {
      if (!isNotFound(err)) throw err
    })
    await loadAlbums()
  } catch (err) {
    libraryError$.next(message(err, 'Could not delete the photo.'))
  }
}

/** Tear down the album layer (on unmount). A pending delete is removed now. */
export function resetLibrary(): void {
  void commitPendingDelete()
  files = []
  replaceGrid([])
  revokeLightbox()
  lightbox$.next(null)
  editorOpen$.next(false)
  albums$.next([])
  selectedAlbum$.next(null)
  uploads$.next(null)
}

/** Dismiss a transient error banner. */
export function clearLibraryError(): void {
  libraryError$.next(undefined)
}

// ─────────────────────────────────────────────────────────────────────────────
// Internals
// ─────────────────────────────────────────────────────────────────────────────

/** Store a file through the write queue, dropping a stale cached thumbnail. */
async function store(path: string, content: Blob | Uint8Array, contentType: string): Promise<void> {
  await queued(() => writeFile(path, content, contentType))
  void loadPlan()
  // A rewritten thumbnail must not keep being served from its stale cached URL.
  const stale = gridThumbUrls.get(path)
  if (stale) {
    URL.revokeObjectURL(stale)
    gridThumbUrls.delete(path)
  }
}

/** Remove a file through the write queue. */
async function remove(path: string): Promise<void> {
  await queued(() => removeFile(path))
  void loadPlan()
}

/** Store the file at `from` at `to`, then remove `from`. */
async function move(from: string, to: string): Promise<void> {
  const bytes = await readFile(from)
  await store(to, bytes, 'application/octet-stream')
  await remove(from)
}

/** Run `write` once every earlier write has settled. */
function queued<T>(write: () => Promise<T>): Promise<T> {
  const result = writes.then(write, write)
  writes = result.catch(() => {})
  return result
}

/** The album names: top-level directories that hold a file, without dotfolders. */
function albumsOf(listing: LibraryFile[]): string[] {
  const names = new Set(listing.map((f) => albumOf(f.path)).filter((name): name is string => name !== null))
  return [...names].filter((name) => !name.startsWith('.')).sort()
}

/** The album a path is directly in, or `null` for a file at the root or deeper down. */
function albumOf(path: string): string | null {
  const names = path.split('/').slice(1)
  return names.length === 2 ? names[0] : null
}

/** Whether a path names a photo rather than a marker or another hidden file. */
function isPhoto(path: string): boolean {
  return !baseName(path).startsWith('.')
}

function baseName(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1)
}

function isNotFound(err: unknown): boolean {
  return err instanceof RemoteStorageError && err.code === 'NotFound'
}

function message(err: unknown, fallback: string): string {
  return err instanceof Error ? err.message : fallback
}

/** Replace the grid, revoking only cached thumbnails whose path left the grid. */
function replaceGrid(items: GridItem[]): void {
  const keep = new Set(items.map((i) => i.thumbPath))
  for (const [thumbPath, url] of gridThumbUrls) {
    if (!keep.has(thumbPath)) {
      URL.revokeObjectURL(url)
      gridThumbUrls.delete(thumbPath)
    }
  }
  entries$.next(items)
}

function revokeLightbox(): void {
  const current = lightbox$.getValue()
  if (current) URL.revokeObjectURL(current.url)
}
