// SPDX-License-Identifier: GPL-3.0-only
//
// A side panel with the storage plans, opened from the profile menu or the
// sidebar's storage bar.

import { useEffect } from 'react'
import { X } from 'lucide-react'
import { usePlan } from '@/state/library.state'
import { closePlanPanel, usePlanPanelOpen } from '@/state/ui.state'
import { PlanPicker } from '@/components/PlanPicker'
import { StorageUsage } from '@/components/StorageUsage'

export function PlanPanel() {
  const open = usePlanPanelOpen()
  const plan = usePlan()

  useEffect(() => {
    if (!open) return
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') closePlanPanel()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open])

  if (!open || !plan) return null

  return (
    <>
      <div className="fixed inset-0 z-40 bg-surface-overlay" onClick={closePlanPanel} aria-hidden="true" />
      <aside
        className="fixed inset-y-0 right-0 z-50 w-full max-w-xl overflow-y-auto bg-surface-container p-6 shadow-2 sm:rounded-l-container"
        aria-label="Storage plan"
        data-testid="plan-panel"
      >
        <div className="flex items-center justify-between">
          <h2 className="text-lg font-semibold leading-tight text-primary">Storage plan</h2>
          <button
            type="button"
            onClick={closePlanPanel}
            aria-label="Close"
            className="cursor-pointer rounded-full p-2 text-secondary transition-colors hover:bg-selection-container-hover hover:text-secondary-hover"
          >
            <X className="h-5 w-5" />
          </button>
        </div>
        <div className="mt-4">
          <StorageUsage plan={plan} />
        </div>
        <div className="mt-6">
          <PlanPicker plan={plan} onDone={closePlanPanel} />
        </div>
      </aside>
    </>
  )
}
