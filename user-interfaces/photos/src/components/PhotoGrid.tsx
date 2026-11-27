// SPDX-License-Identifier: GPL-3.0-only
//
// The photo grid: thumbnails grouped by the day they were stored, newest first.
// Listing downloads thumbnails only; clicking a photo opens it full-resolution
// in the lightbox, where it can be renamed, edited or deleted.

import { useMemo } from 'react'
import { ImageOff, Images } from 'lucide-react'
import { openPhoto, useEntries, useGridLoading, useSelectedAlbum, DEFAULT_ALBUM, type GridItem } from '@/state/album.state'
import { dayLabel, groupByDay } from '@/lib/group-by-day'
import { Spinner } from '@/components/ui/Spinner'
import { UploadButton } from '@/components/UploadButton'

export function PhotoGrid() {
  const entries = useEntries()
  const loading = useGridLoading()
  const selectedAlbum = useSelectedAlbum()
  const groups = useMemo(() => groupByDay(entries), [entries])

  if (loading && entries.length === 0) {
    return (
      <div className="flex items-center justify-center gap-3 py-24 text-sm text-secondary">
        <Spinner size="sm" /> Loading photos…
      </div>
    )
  }

  if (entries.length === 0) {
    return (
      <div className="flex flex-col items-center py-24 text-center">
        <div className="rounded-full bg-surface-container p-4">
          <Images className="h-6 w-6 text-tertiary" />
        </div>
        <h2 className="mt-4 text-lg font-semibold leading-tight text-primary">
          {selectedAlbum ? `${selectedAlbum} is empty` : 'No photos yet'}
        </h2>
        <p className="mt-1 max-w-sm text-sm text-secondary">
          Drop photos here or upload them.
          {!selectedAlbum && ` They go into ${DEFAULT_ALBUM}.`}
        </p>
        <div className="mt-6">
          <UploadButton />
        </div>
      </div>
    )
  }

  return (
    <div className="space-y-6" data-testid="photo-grid">
      {groups.map((group) => (
        <section key={group.day}>
          <h2 className="sticky top-16 z-10 bg-surface-main py-2 text-sm font-medium text-primary">
            {dayLabel(group.day)}
          </h2>
          <div className="grid grid-cols-[repeat(auto-fill,minmax(120px,1fr))] gap-1 sm:grid-cols-[repeat(auto-fill,minmax(180px,1fr))]">
            {group.items.map((item) => (
              <PhotoCell key={item.path} item={item} />
            ))}
          </div>
        </section>
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
      className="group relative aspect-square cursor-pointer overflow-hidden bg-surface-container"
      data-testid="photo-cell"
    >
      {item.thumbUrl ? (
        <img
          src={item.thumbUrl}
          alt={item.name}
          loading="lazy"
          className="h-full w-full object-cover transition-transform duration-200 group-hover:scale-[1.03]"
        />
      ) : (
        <span className="flex h-full w-full items-center justify-center text-tertiary">
          <ImageOff className="h-6 w-6" />
        </span>
      )}
    </button>
  )
}
