// SPDX-License-Identifier: GPL-3.0-only
//
// The sidebar: every photo, the albums, a new-album field, and how much of the
// plan the library uses. A drawer on narrow screens.

import { useState } from 'react'
import { Folder, Images, Plus } from 'lucide-react'
import { createAlbum, selectAlbum, useAlbums, useSelectedAlbum } from '@/state/album.state'
import { usePlan } from '@/state/library.state'
import { closeSidebar, openPlanPanel, useSidebarOpen } from '@/state/ui.state'
import { StorageUsage } from '@/components/StorageUsage'
import { Input } from '@/components/ui/Input'
import { cn } from '@/utils/cn'

export function Sidebar() {
  const open = useSidebarOpen()

  return (
    <>
      {open && <div className="fixed inset-0 z-30 bg-surface-overlay md:hidden" onClick={closeSidebar} aria-hidden="true" />}
      <aside
        className={cn(
          'fixed bottom-0 left-0 top-16 z-30 flex w-64 flex-col bg-surface-main px-3 py-4 transition-transform md:sticky md:top-16 md:h-[calc(100vh-4rem)] md:translate-x-0',
          open ? 'translate-x-0' : '-translate-x-full',
        )}
        data-testid="sidebar"
      >
        <SidebarContent />
      </aside>
    </>
  )
}

function SidebarContent() {
  const albums = useAlbums()
  const selected = useSelectedAlbum()
  const plan = usePlan()

  function choose(album: string | null) {
    closeSidebar()
    void selectAlbum(album)
  }

  return (
    <>
      <nav className="flex flex-1 flex-col gap-1 overflow-y-auto" data-testid="album-bar">
        <NavItem active={selected === null} onClick={() => choose(null)} icon={<Images className="h-5 w-5" />}>
          Photos
        </NavItem>

        <p className="mt-4 px-3 pb-1 text-xs font-medium text-tertiary">Albums</p>
        {albums.map((album) => (
          <NavItem
            key={album}
            active={album === selected}
            onClick={() => choose(album)}
            icon={<Folder className="h-5 w-5" />}
            testId="album-tab"
          >
            {album}
          </NavItem>
        ))}
        <NewAlbum />
      </nav>

      {plan && (
        <button
          type="button"
          onClick={() => {
            closeSidebar()
            openPlanPanel()
          }}
          className="mt-4 cursor-pointer rounded-nested px-3 py-3 text-left transition-colors hover:bg-selection-container-hover"
          data-testid="sidebar-storage"
        >
          <p className="mb-2 text-sm font-medium text-primary">Storage</p>
          <StorageUsage plan={plan} />
        </button>
      )}
    </>
  )
}

function NavItem({
  active,
  onClick,
  icon,
  testId,
  children,
}: {
  active: boolean
  onClick: () => void
  icon: React.ReactNode
  testId?: string
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-current={active ? 'page' : undefined}
      className={cn(
        'flex cursor-pointer items-center gap-3 rounded-full px-3 py-2 text-left text-sm font-medium transition-colors',
        active ? 'bg-selection-container-active text-primary' : 'text-secondary hover:bg-selection-container-hover',
      )}
      data-testid={testId}
    >
      <span className="shrink-0">{icon}</span>
      <span className="truncate">{children}</span>
    </button>
  )
}

function NewAlbum() {
  const [adding, setAdding] = useState(false)
  const [name, setName] = useState('')

  function close() {
    setName('')
    setAdding(false)
  }

  function submit() {
    const value = name.trim()
    if (value) void createAlbum(value)
    close()
  }

  if (!adding) {
    return (
      <button
        type="button"
        onClick={() => setAdding(true)}
        className="flex cursor-pointer items-center gap-3 rounded-full px-3 py-2 text-sm font-medium text-secondary transition-colors hover:bg-selection-container-hover"
        data-testid="new-album-button"
      >
        <Plus className="h-5 w-5" /> New album
      </button>
    )
  }

  return (
    <div className="px-1 py-1">
      <Input
        autoFocus
        value={name}
        placeholder="Album name"
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') submit()
          if (e.key === 'Escape') close()
        }}
        onBlur={submit}
        data-testid="new-album-input"
      />
    </div>
  )
}
