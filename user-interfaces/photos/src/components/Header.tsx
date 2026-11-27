// SPDX-License-Identifier: GPL-3.0-only
//
// The top bar: the sidebar toggle (narrow screens), the app name, upload, the
// chain status and the profile menu.

import { Image, Menu } from 'lucide-react'
import { isInHost } from '@/lib/host-account'
import { hasLapsed, hasLibrary, usePlan } from '@/state/library.state'
import { toggleSidebar } from '@/state/ui.state'
import { ChainStatus } from '@/components/ChainStatus'
import { ProfileMenu } from '@/components/ProfileMenu'
import { UploadButton } from '@/components/UploadButton'

export function Header() {
  const plan = usePlan()
  const library = hasLibrary(plan) && !hasLapsed(plan)
  const inHost = isInHost()

  return (
    <header className="sticky top-0 z-40 flex h-16 items-center gap-2 bg-surface-main px-2 sm:px-4">
      {library && (
        <button
          type="button"
          onClick={toggleSidebar}
          aria-label="Albums"
          className="cursor-pointer rounded-full p-2 text-secondary transition-colors hover:bg-selection-container-hover hover:text-secondary-hover md:hidden"
        >
          <Menu className="h-5 w-5" />
        </button>
      )}
      <div className="flex items-center gap-2 px-2">
        <Image className="h-6 w-6 text-primary" />
        <span className="text-lg font-semibold leading-tight text-primary">Photos</span>
      </div>

      <div className="flex-1" />

      {library && <UploadButton compact />}
      {inHost && <ChainStatus />}
      {inHost && <ProfileMenu />}
    </header>
  )
}
