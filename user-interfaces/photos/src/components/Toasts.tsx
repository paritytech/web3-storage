// SPDX-License-Identifier: GPL-3.0-only
//
// Bottom-right notices: upload progress, a deleted photo with Undo, and library
// errors.

import { AlertTriangle, X } from 'lucide-react'
import {
  clearLibraryError,
  undoDelete,
  useLibraryError,
  usePendingDelete,
  useUploads,
} from '@/state/album.state'
import { Spinner } from '@/components/ui/Spinner'

export function Toasts() {
  const uploads = useUploads()
  const pendingDelete = usePendingDelete()
  const error = useLibraryError()

  if (!uploads && !pendingDelete && !error) return null

  return (
    <div className="fixed bottom-4 right-4 z-[70] flex w-[min(24rem,calc(100vw-2rem))] flex-col gap-2" aria-live="polite">
      {uploads && (
        <Toast testId="upload-status">
          <Spinner size="sm" />
          <span className="min-w-0 flex-1 truncate">
            Uploading {Math.min(uploads.done + 1, uploads.total)} of {uploads.total}
            {uploads.current && <span className="text-secondary"> · {uploads.current}</span>}
          </span>
        </Toast>
      )}

      {pendingDelete && (
        <Toast testId="delete-toast">
          <span className="min-w-0 flex-1 truncate">Deleted {pendingDelete.name}</span>
          <button
            type="button"
            onClick={() => void undoDelete()}
            className="shrink-0 cursor-pointer rounded-small px-2 py-1 text-sm font-semibold text-primary transition-colors hover:bg-selection-container-hover"
            data-testid="delete-undo"
          >
            Undo
          </button>
        </Toast>
      )}

      {error && (
        <Toast testId="library-error">
          <AlertTriangle className="h-4 w-4 shrink-0 text-error" />
          <span className="min-w-0 flex-1 break-words">{error}</span>
          <button
            type="button"
            onClick={() => clearLibraryError()}
            aria-label="Dismiss"
            className="shrink-0 cursor-pointer rounded-full p-1 text-secondary transition-colors hover:bg-selection-container-hover hover:text-secondary-hover"
          >
            <X className="h-4 w-4" />
          </button>
        </Toast>
      )}
    </div>
  )
}

function Toast({ testId, children }: { testId: string; children: React.ReactNode }) {
  return (
    <div
      className="flex items-center gap-3 rounded-nested bg-surface-nested px-4 py-3 text-sm text-primary shadow-2"
      data-testid={testId}
    >
      {children}
    </div>
  )
}
