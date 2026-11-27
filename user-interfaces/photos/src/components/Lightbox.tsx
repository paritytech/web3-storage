// SPDX-License-Identifier: GPL-3.0-only
//
// Full-resolution photo viewer. Shows the object URL produced by `openPhoto`
// (a fresh download of the file) over the whole window, with the previous and
// next photo of the grid on ← and →, and rename, edit and delete. Closing
// releases the object URL (`closePhoto`).

import { useEffect, useState } from 'react'
import { ArrowLeft, ChevronLeft, ChevronRight, Pencil, SlidersHorizontal, Trash2 } from 'lucide-react'
import {
  adjacentPhoto,
  closePhoto,
  deleteWithUndo,
  openAdjacentPhoto,
  openEditor,
  openPhoto,
  renamePhoto,
  useEditorOpen,
  useEntries,
  useLightbox,
  useLightboxLoading,
} from '@/state/album.state'
import { Input } from '@/components/ui/Input'
import { Spinner } from '@/components/ui/Spinner'

export function Lightbox() {
  const photo = useLightbox()
  const loading = useLightboxLoading()
  const editorOpen = useEditorOpen()
  // Re-render when the grid changes, so the arrows reflect the current grid.
  useEntries()
  const [renaming, setRenaming] = useState(false)
  const open = photo !== null || loading
  const hasPrevious = adjacentPhoto(-1) !== undefined
  const hasNext = adjacentPhoto(1) !== undefined

  // Escape closes, ← and → move — unless the editor is on top (Escape is its
  // cancel) or the name field has the keys.
  useEffect(() => {
    if (!open || editorOpen || renaming) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') closePhoto()
      if (e.key === 'ArrowLeft') void openAdjacentPhoto(-1)
      if (e.key === 'ArrowRight') void openAdjacentPhoto(1)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, editorOpen, renaming])

  useEffect(() => setRenaming(false), [photo?.item.path])

  if (!open) return null

  function remove() {
    if (!photo) return
    const next = adjacentPhoto(1) ?? adjacentPhoto(-1)
    deleteWithUndo(photo.item)
    if (next) void openPhoto(next)
    else closePhoto()
  }

  return (
    <div className="fixed inset-0 z-50 flex flex-col bg-surface-main" data-testid="lightbox">
      <div className="flex h-16 shrink-0 items-center gap-2 px-2 sm:px-4">
        <IconButton label="Close" onClick={closePhoto}>
          <ArrowLeft className="h-5 w-5" />
        </IconButton>
        <div className="min-w-0 flex-1">
          {photo && (renaming ? <RenameField key={photo.item.path} onDone={() => setRenaming(false)} /> : (
            <p className="truncate text-sm text-secondary">{photo.name}</p>
          ))}
        </div>
        {photo && !renaming && (
          <>
            <IconButton label="Rename" onClick={() => setRenaming(true)} testId="photo-rename">
              <Pencil className="h-5 w-5" />
            </IconButton>
            <IconButton label="Edit" onClick={openEditor} testId="lightbox-edit">
              <SlidersHorizontal className="h-5 w-5" />
            </IconButton>
            <IconButton label="Delete" onClick={remove} testId="photo-delete">
              <Trash2 className="h-5 w-5" />
            </IconButton>
          </>
        )}
      </div>

      <div className="relative flex min-h-0 flex-1 items-center justify-center px-2 pb-4 sm:px-16">
        {hasPrevious && (
          <NavArrow side="left" label="Previous photo" onClick={() => void openAdjacentPhoto(-1)}>
            <ChevronLeft className="h-6 w-6" />
          </NavArrow>
        )}
        {loading && !photo ? (
          <div className="flex items-center gap-3 text-secondary">
            <Spinner /> Loading photo…
          </div>
        ) : (
          photo && (
            <img
              src={photo.url}
              alt={photo.name}
              className="max-h-full max-w-full object-contain"
              data-testid="lightbox-image"
            />
          )
        )}
        {loading && photo && (
          <div className="absolute right-6 top-2">
            <Spinner size="sm" />
          </div>
        )}
        {hasNext && (
          <NavArrow side="right" label="Next photo" onClick={() => void openAdjacentPhoto(1)}>
            <ChevronRight className="h-6 w-6" />
          </NavArrow>
        )}
      </div>
    </div>
  )
}

function RenameField({ onDone }: { onDone: () => void }) {
  const photo = useLightbox()
  const name = photo?.name ?? ''
  const dot = name.lastIndexOf('.')
  const stem = dot > 0 ? name.slice(0, dot) : name
  const ext = dot > 0 ? name.slice(dot) : ''
  const [value, setValue] = useState(stem)

  async function submit() {
    const next = value.trim()
    onDone()
    if (photo && next && next !== stem) await renamePhoto(photo.item, next + ext)
  }

  return (
    <div className="flex max-w-sm items-center gap-1">
      <Input
        autoFocus
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') void submit()
          if (e.key === 'Escape') onDone()
        }}
        onBlur={() => void submit()}
        className="h-8"
        data-testid="photo-rename-input"
      />
      {ext && <span className="shrink-0 text-sm text-tertiary">{ext}</span>}
    </div>
  )
}

function IconButton({
  label,
  onClick,
  testId,
  children,
}: {
  label: string
  onClick: () => void
  testId?: string
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      title={label}
      className="cursor-pointer rounded-full p-2 text-secondary transition-colors hover:bg-selection-container-hover hover:text-secondary-hover"
      data-testid={testId}
    >
      {children}
    </button>
  )
}

function NavArrow({
  side,
  label,
  onClick,
  children,
}: {
  side: 'left' | 'right'
  label: string
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      className={`absolute top-1/2 z-10 -translate-y-1/2 cursor-pointer rounded-full bg-surface-container p-2 text-primary shadow-1 transition-colors hover:bg-selection-container-hover ${
        side === 'left' ? 'left-2 sm:left-4' : 'right-2 sm:right-4'
      }`}
    >
      {children}
    </button>
  )
}
