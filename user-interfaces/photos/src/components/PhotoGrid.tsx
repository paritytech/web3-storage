// SPDX-License-Identifier: GPL-3.0-only
//
// M6 — photo grid. Renders the selected album from its thumbnails (kilobytes per
// cell); clicking a photo opens it full-resolution in the lightbox. Listing an
// album therefore downloads thumbnails, never the full blobs.

import { ImageOff, ImageIcon } from 'lucide-react'
import { useEntries, useGridLoading, useSelectedAlbum, openPhoto, type GridItem } from '@/state/album.state'
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

function PhotoCell({ item }: { item: GridItem }) {
  return (
    <button
      type="button"
      onClick={() => void openPhoto(item)}
      title={item.name}
      data-testid="photo-cell"
      className="group relative aspect-square overflow-hidden rounded-md border border-gray-800 bg-gray-900/60 focus:outline-none focus:ring-1 focus:ring-purple-500"
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
      <div className="absolute inset-x-0 bottom-0 flex items-center justify-between gap-2 bg-gradient-to-t from-black/70 to-transparent px-2 py-1.5 text-left">
        <span className="truncate text-[11px] text-gray-200">{item.name}</span>
        <span className="shrink-0 text-[10px] text-gray-400">{formatBytes(item.size)}</span>
      </div>
    </button>
  )
}

function Empty({ icon, children }: { icon: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 rounded-md border border-dashed border-gray-800 bg-gray-900/30 py-12 text-center text-sm text-gray-400">
      <div className="text-gray-600">{icon}</div>
      <p>{children}</p>
    </div>
  )
}
