// SPDX-License-Identifier: GPL-3.0-only
//
// M6 — album selector. Renders the drive's albums as tabs and an inline
// "New album" affordance. Album selection and creation go through
// `album.state.ts`; creating one recomputes + anchors the metadata root.

import { useState } from 'react'
import { FolderPlus, Check, X } from 'lucide-react'
import { useAlbums, useSelectedAlbum, selectAlbum, createAlbum } from '@/state/album.state'
import { Button } from '@/components/ui/Button'
import { Input } from '@/components/ui/Input'
import { cn } from '@/utils/cn'

export function AlbumBar({ busy }: { busy?: boolean }) {
  const albums = useAlbums()
  const selected = useSelectedAlbum()
  const [adding, setAdding] = useState(false)
  const [name, setName] = useState('')

  function submit() {
    const value = name.trim()
    if (!value) return
    void createAlbum(value)
    setName('')
    setAdding(false)
  }

  return (
    <div className="flex flex-wrap items-center gap-2" data-testid="album-bar">
      {albums.map((album) => (
        <button
          key={album}
          type="button"
          disabled={busy}
          onClick={() => void selectAlbum(album)}
          data-testid="album-tab"
          className={cn(
            'rounded-md border px-3 py-1.5 text-sm transition-colors disabled:opacity-50',
            album === selected
              ? 'border-purple-600 bg-purple-600/20 text-purple-200'
              : 'border-gray-800 bg-gray-900/40 text-gray-300 hover:bg-gray-800',
          )}
        >
          {album}
        </button>
      ))}

      {adding ? (
        <div className="flex items-center gap-1.5">
          <Input
            autoFocus
            value={name}
            placeholder="Album name"
            disabled={busy}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') submit()
              if (e.key === 'Escape') {
                setName('')
                setAdding(false)
              }
            }}
            className="h-8 w-40"
            data-testid="new-album-input"
          />
          <Button size="icon" className="h-8 w-8" onClick={submit} disabled={busy} data-testid="new-album-save">
            <Check className="h-4 w-4" />
          </Button>
          <Button
            size="icon"
            variant="ghost"
            className="h-8 w-8"
            onClick={() => {
              setName('')
              setAdding(false)
            }}
          >
            <X className="h-4 w-4" />
          </Button>
        </div>
      ) : (
        <Button
          size="sm"
          variant="outline"
          onClick={() => setAdding(true)}
          disabled={busy}
          data-testid="new-album-button"
        >
          <FolderPlus className="mr-1.5 h-4 w-4" /> New album
        </Button>
      )}
    </div>
  )
}
