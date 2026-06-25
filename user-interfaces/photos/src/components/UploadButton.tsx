// SPDX-License-Identifier: GPL-3.0-only
//
// M6 — photo upload. A file picker (images, multi-select) that uploads into the
// selected album via `album.state.ts` (full photo + client-generated thumbnail),
// then surfaces the batch progress and the on-chain anchor status.

import { useRef } from 'react'
import { Upload } from 'lucide-react'
import { uploadPhotos, useUploads, useAnchorStatus, useSelectedAlbum } from '@/state/album.state'
import { Button } from '@/components/ui/Button'
import { Spinner } from '@/components/ui/Spinner'

const ANCHOR_LABEL: Record<string, string> = {
  recomputing: 'Recomputing library root…',
  anchoring: 'Anchoring root on-chain…',
}

export function UploadButton() {
  const selectedAlbum = useSelectedAlbum()
  const uploads = useUploads()
  const anchor = useAnchorStatus()
  const inputRef = useRef<HTMLInputElement>(null)

  const busy = uploads !== null || anchor.stage === 'recomputing' || anchor.stage === 'anchoring'

  function onPick(e: React.ChangeEvent<HTMLInputElement>) {
    const files = Array.from(e.target.files ?? [])
    e.target.value = '' // allow re-picking the same file
    if (files.length > 0) void uploadPhotos(files)
  }

  return (
    <div className="flex items-center gap-3">
      <input
        ref={inputRef}
        type="file"
        accept="image/*"
        multiple
        className="hidden"
        onChange={onPick}
        data-testid="upload-input"
      />
      <Button
        size="sm"
        onClick={() => inputRef.current?.click()}
        disabled={!selectedAlbum || busy}
        data-testid="upload-button"
      >
        <Upload className="mr-1.5 h-4 w-4" /> Upload
      </Button>

      {busy && (
        <span className="flex items-center gap-2 text-xs text-gray-400" data-testid="upload-status">
          <Spinner size="sm" />
          {uploads
            ? `Uploading ${Math.min(uploads.done + 1, uploads.total)}/${uploads.total}${
                uploads.current ? ` — ${uploads.current}` : ''
              }`
            : (ANCHOR_LABEL[anchor.stage] ?? '')}
        </span>
      )}
    </div>
  )
}
