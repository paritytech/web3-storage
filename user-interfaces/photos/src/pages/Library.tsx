// SPDX-License-Identifier: GPL-3.0-only
//
// The library page. Photos keeps its library in the TrUAPI host's remote
// storage, so it reads the storage plan and renders either the plan choice (no
// plan yet, or a lapsed one) or the library: sidebar, photo grid, lightbox.

import { useEffect, useState } from 'react'
import { AlertTriangle, Image } from 'lucide-react'
import { isInHost } from '@/lib/host-account'
import { hasLapsed, hasLibrary, loadPlan, usePlan, usePlanError } from '@/state/library.state'
import { initLibrary, resetLibrary, uploadPhotos, useUploads } from '@/state/album.state'
import { Button } from '@/components/ui/Button'
import { Spinner } from '@/components/ui/Spinner'
import { PlanPicker } from '@/components/PlanPicker'
import { PlanPanel } from '@/components/PlanPanel'
import { Sidebar } from '@/components/Sidebar'
import { PhotoGrid } from '@/components/PhotoGrid'
import { Lightbox } from '@/components/Lightbox'
import { PhotoEditor } from '@/components/PhotoEditor'
import { Toasts } from '@/components/Toasts'
import { cn } from '@/utils/cn'

export function Library() {
  const inHost = isInHost()
  const plan = usePlan()
  const planError = usePlanError()
  const library = hasLibrary(plan) && !hasLapsed(plan)

  useEffect(() => {
    if (inHost) void loadPlan()
  }, [inHost])

  // Load the albums once the library exists, including right after its plan is
  // created in this session; release object URLs and caches on unmount.
  useEffect(() => {
    if (library) void initLibrary()
  }, [library])
  useEffect(() => () => resetLibrary(), [])

  // ── Outside a host there is no library ──
  if (!inHost) {
    return (
      <Centered>
        <div className="max-w-md rounded-container bg-surface-container p-8 text-center">
          <div className="mx-auto w-fit rounded-full bg-surface-nested p-4">
            <Image className="h-6 w-6 text-secondary" />
          </div>
          <h1 className="mt-4 text-lg font-semibold leading-tight text-primary">Open Photos in a host</h1>
          <p className="mt-2 text-sm text-secondary">
            Photos keeps your library through a TrUAPI host. Open it in Polkadot Desktop, Polkadot Web or{' '}
            <code className="text-primary">truapi-host dev</code>.
          </p>
        </div>
      </Centered>
    )
  }

  // ── Plan read failed ──
  if (planError) {
    return (
      <Centered>
        <div className="max-w-lg rounded-container bg-surface-container p-6">
          <h1 className="flex items-center gap-2 font-semibold leading-tight text-error">
            <AlertTriangle className="h-5 w-5" /> Could not read your library
          </h1>
          <p className="mt-2 text-sm text-secondary">Your host could not read the storage plan.</p>
          <pre className="mt-3 whitespace-pre-wrap break-words text-xs text-tertiary">{planError}</pre>
          <Button className="mt-4" size="sm" variant="secondary" onClick={() => void loadPlan()}>
            Try again
          </Button>
        </div>
      </Centered>
    )
  }

  // ── Reading the plan ──
  if (!plan) {
    return (
      <Centered>
        <div className="flex items-center gap-3 text-secondary">
          <Spinner /> Reading your library…
        </div>
      </Centered>
    )
  }

  // ── No library yet, or its plan lapsed: choose a plan ──
  if (!library) {
    const lapsed = hasLapsed(plan)
    return (
      <main className="mx-auto max-w-3xl px-4 pb-16 pt-12">
        <h1 className="text-3xl font-semibold leading-tight text-primary">
          {lapsed ? 'Renew your library' : 'Choose a plan for your photos'}
        </h1>
        <p className="mt-2 text-secondary">
          {lapsed
            ? 'Your plan ended, so the storage provider no longer has to keep your photos. Choose a plan to open your library again.'
            : 'Your photos are kept by a storage provider your host picks. Your host asks you to approve the price before you pay.'}
        </p>
        <div className="mt-8">
          <PlanPicker plan={plan} />
        </div>
        <Toasts />
      </main>
    )
  }

  // ── The library ──
  return (
    <div className="flex">
      <Sidebar />
      <DropZone>
        <PhotoGrid />
      </DropZone>
      <PlanPanel />
      <Lightbox />
      <PhotoEditor />
      <Toasts />
    </div>
  )
}

/** The main area. Dropping image files on it uploads them. */
function DropZone({ children }: { children: React.ReactNode }) {
  const uploads = useUploads()
  const [dragging, setDragging] = useState(false)

  function hasFiles(event: React.DragEvent) {
    return event.dataTransfer.types.includes('Files')
  }

  return (
    <main
      className={cn(
        'relative min-h-[calc(100vh-4rem)] min-w-0 flex-1 px-2 pb-16 sm:px-4',
        dragging && 'outline outline-2 -outline-offset-8 outline-indicator',
      )}
      onDragOver={(event) => {
        if (!hasFiles(event) || uploads) return
        event.preventDefault()
        setDragging(true)
      }}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDragging(false)
      }}
      onDrop={(event) => {
        if (!hasFiles(event)) return
        event.preventDefault()
        setDragging(false)
        const files = Array.from(event.dataTransfer.files).filter((file) => file.type.startsWith('image/'))
        if (files.length > 0 && !uploads) void uploadPhotos(files)
      }}
      data-testid="drop-zone"
    >
      {children}
    </main>
  )
}

function Centered({ children }: { children: React.ReactNode }) {
  return <main className="flex justify-center px-4 pt-24">{children}</main>
}
