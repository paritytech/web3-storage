// SPDX-License-Identifier: GPL-3.0-only
//
// Photo upload. A file picker (images, multi-select) that uploads into the
// selected album, or into the default album from the all-photos view.

import { useRef } from 'react'
import { Upload } from 'lucide-react'
import { uploadPhotos, useUploads } from '@/state/album.state'
import { Button } from '@/components/ui/Button'

export function UploadButton({ compact }: { compact?: boolean }) {
  const uploads = useUploads()
  const inputRef = useRef<HTMLInputElement>(null)

  function onPick(e: React.ChangeEvent<HTMLInputElement>) {
    const files = Array.from(e.target.files ?? [])
    e.target.value = '' // allow re-picking the same file
    if (files.length > 0) void uploadPhotos(files)
  }

  return (
    <>
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
        variant={compact ? 'ghost' : 'default'}
        onClick={() => inputRef.current?.click()}
        disabled={uploads !== null}
        data-testid="upload-button"
      >
        <Upload className="h-4 w-4 sm:mr-1.5" />
        <span className={compact ? 'hidden sm:inline' : undefined}>Upload</span>
      </Button>
    </>
  )
}
