// SPDX-License-Identifier: GPL-3.0-only
//
// M6 — full-resolution photo viewer. Shows the object URL produced by
// `openPhoto` (a fresh `GET /fs/.../file?path=` download) in a modal overlay.
// Closing releases the object URL (`closePhoto`).

import { useEffect } from 'react'
import { X, Pencil } from 'lucide-react'
import {
  useLightbox,
  useLightboxLoading,
  useEditorOpen,
  closePhoto,
  openEditor,
} from '@/state/album.state'
import { Spinner } from '@/components/ui/Spinner'

export function Lightbox() {
  const photo = useLightbox()
  const loading = useLightboxLoading()
  const editorOpen = useEditorOpen()
  const open = photo !== null || loading

  // Close on Escape while open — unless the editor is on top, where Escape is
  // the editor's own cancel (back to the lightbox).
  useEffect(() => {
    if (!open || editorOpen) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') closePhoto()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, editorOpen])

  if (!open) return null

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 p-6"
      onClick={closePhoto}
      data-testid="lightbox"
    >
      <div className="absolute right-4 top-4 flex items-center gap-1" onClick={(e) => e.stopPropagation()}>
        {photo && (
          <button
            type="button"
            onClick={openEditor}
            aria-label="Edit"
            className="flex items-center gap-1.5 rounded-md px-3 py-2 text-sm text-gray-300 hover:bg-white/10 hover:text-white"
            data-testid="lightbox-edit"
          >
            <Pencil className="h-5 w-5" /> Edit
          </button>
        )}
        <button
          type="button"
          onClick={closePhoto}
          aria-label="Close"
          className="rounded-md p-2 text-gray-300 hover:bg-white/10 hover:text-white"
        >
          <X className="h-6 w-6" />
        </button>
      </div>

      {loading ? (
        <div className="flex items-center gap-3 text-gray-300">
          <Spinner /> Loading full resolution…
        </div>
      ) : (
        photo && (
          <figure
            className="flex max-h-full max-w-full flex-col items-center gap-3"
            onClick={(e) => e.stopPropagation()}
          >
            <img
              src={photo.url}
              alt={photo.name}
              className="max-h-[80vh] max-w-full rounded-md object-contain"
              data-testid="lightbox-image"
            />
            <figcaption className="text-sm text-gray-400">{photo.name}</figcaption>
          </figure>
        )
      )}
    </div>
  )
}
