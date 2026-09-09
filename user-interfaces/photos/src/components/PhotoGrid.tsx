// SPDX-License-Identifier: GPL-3.0-only
//
// M6 — photo grid. Renders the selected album from its thumbnails (kilobytes per
// cell); clicking a photo opens it full-resolution in the lightbox. Listing an
// album therefore downloads thumbnails, never the full blobs. Each cell also
// offers inline rename and delete on hover.

import { useState } from 'react'
import { ImageOff, ImageIcon, Pencil, Trash2, Check } from 'lucide-react'
import {
  useEntries,
  useGridLoading,
  useSelectedAlbum,
  openPhoto,
  renamePhoto,
  deletePhoto,
  type GridItem,
} from '@/state/album.state'
import { Button } from '@/components/ui/Button'
import { Input } from '@/components/ui/Input'
import { Spinner } from '@/components/ui/Spinner'
import { formatBytes } from '@/utils/format'

export function PhotoGrid() {
  const entries = useEntries()
  const loading = useGridLoading()
  const selectedAlbum = useSelectedAlbum()

  if (!selectedAlbum) {
    return (
      <Empty icon={<ImageIcon className="h-8 w-8" />}>
        Create an album to start adding photos.
      </Empty>
    )
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center gap-3 py-12 text-sm text-gray-400">
        <Spinner size="sm" /> Loading photos…
      </div>
    )
  }

  if (entries.length === 0) {
    return (
      <Empty icon={<ImageIcon className="h-8 w-8" />}>
        No photos in <span className="text-gray-300">{selectedAlbum}</span> yet — upload some.
      </Empty>
    )
  }

  return (
    <div
      className="grid grid-cols-2 gap-3 sm:grid-cols-3 md:grid-cols-4"
      data-testid="photo-grid"
    >
      {entries.map((item) => (
        <PhotoCell key={item.path} item={item} />
      ))}
    </div>
  )
}

type CellMode = 'idle' | 'renaming' | 'confirmDelete' | 'busy'

function PhotoCell({ item }: { item: GridItem }) {
  const [mode, setMode] = useState<CellMode>('idle')
  const { stem, ext } = splitName(item.name)
  const [value, setValue] = useState(stem)

  async function submitRename() {
    const next = value.trim()
    if (!next || next === stem) {
      setMode('idle')
      setValue(stem)
      return
    }
    setMode('busy')
    await renamePhoto(item, next + ext)
    // On success the grid reloads and this cell unmounts; on error it stays, so
    // drop back to idle (the error banner explains what happened).
    setMode('idle')
  }

  async function confirmDelete() {
    setMode('busy')
    await deletePhoto(item)
    setMode('idle')
  }

  return (
    <div
      className="group relative aspect-square overflow-hidden rounded-md border border-gray-800 bg-gray-900/60"
      data-testid="photo-cell"
    >
      <button
        type="button"
        onClick={() => void openPhoto(item)}
        title={item.name}
        disabled={mode !== 'idle'}
        className="absolute inset-0 focus:outline-none focus:ring-1 focus:ring-purple-500"
      >
        {item.thumbUrl ? (
          <img
            src={item.thumbUrl}
            alt={item.name}
            loading="lazy"
            className="h-full w-full object-cover transition-transform group-hover:scale-105"
          />
        ) : (
          <div className="flex h-full w-full items-center justify-center text-gray-600">
            <ImageOff className="h-6 w-6" />
          </div>
        )}
      </button>

      {/* Hover action bar */}
      {mode === 'idle' && (
        <div
          className="absolute right-1 top-1 z-10 flex gap-1 opacity-0 transition-opacity group-hover:opacity-100"
          onClick={(e) => e.stopPropagation()}
        >
          <button
            type="button"
            aria-label="Rename"
            data-testid="photo-rename"
            onClick={() => {
              setValue(stem)
              setMode('renaming')
            }}
            className="rounded-md bg-black/60 p-1.5 text-gray-200 hover:bg-black/80 hover:text-white"
          >
            <Pencil className="h-3.5 w-3.5" />
          </button>
          <button
            type="button"
            aria-label="Delete"
            data-testid="photo-delete"
            onClick={() => setMode('confirmDelete')}
            className="rounded-md bg-black/60 p-1.5 text-gray-200 hover:bg-red-600 hover:text-white"
          >
            <Trash2 className="h-3.5 w-3.5" />
          </button>
        </div>
      )}

      {/* Delete confirmation */}
      {mode === 'confirmDelete' && (
        <div
          className="absolute inset-0 z-20 flex flex-col items-center justify-center gap-2 bg-black/75 p-2 text-center"
          onClick={(e) => e.stopPropagation()}
        >
          <span className="text-xs text-gray-200">Delete this photo?</span>
          <div className="flex gap-2">
            <Button
              size="sm"
              variant="destructive"
              onClick={() => void confirmDelete()}
              data-testid="photo-delete-confirm"
            >
              Delete
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setMode('idle')}>
              Cancel
            </Button>
          </div>
        </div>
      )}

      {/* Busy spinner (rename/delete in flight) */}
      {mode === 'busy' && (
        <div className="absolute inset-0 z-20 flex items-center justify-center bg-black/50">
          <Spinner size="sm" />
        </div>
      )}

      {/* Caption / rename row */}
      <div
        className="absolute inset-x-0 bottom-0 z-10 flex items-center justify-between gap-1 bg-gradient-to-t from-black/70 to-transparent px-2 py-1.5 text-left"
        onClick={(e) => e.stopPropagation()}
      >
        {mode === 'renaming' ? (
          <div className="flex w-full items-center gap-1">
            <Input
              autoFocus
              value={value}
              onChange={(e) => setValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void submitRename()
                if (e.key === 'Escape') {
                  setValue(stem)
                  setMode('idle')
                }
              }}
              className="h-7 flex-1 px-2 text-[11px]"
              data-testid="photo-rename-input"
            />
            {ext && <span className="shrink-0 text-[11px] text-gray-400">{ext}</span>}
            <button
              type="button"
              aria-label="Save name"
              onClick={() => void submitRename()}
              data-testid="photo-rename-save"
              className="shrink-0 rounded p-1 text-gray-200 hover:bg-white/10 hover:text-white"
            >
              <Check className="h-3.5 w-3.5" />
            </button>
          </div>
        ) : (
          <>
            <span className="truncate text-[11px] text-gray-200">{item.name}</span>
            <span className="shrink-0 text-[10px] text-gray-400">{formatBytes(item.size)}</span>
          </>
        )}
      </div>
    </div>
  )
}

/** Split a filename into its base name and extension (the dot-prefixed suffix, or ''). */
function splitName(name: string): { stem: string; ext: string } {
  const dot = name.lastIndexOf('.')
  return dot > 0 ? { stem: name.slice(0, dot), ext: name.slice(dot) } : { stem: name, ext: '' }
}

function Empty({ icon, children }: { icon: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 rounded-md border border-dashed border-gray-800 bg-gray-900/30 py-12 text-center text-sm text-gray-400">
      <div className="text-gray-600">{icon}</div>
      <p>{children}</p>
    </div>
  )
}
