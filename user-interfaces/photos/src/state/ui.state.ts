// SPDX-License-Identifier: GPL-3.0-only
//
// UI State — which panels are open.

import { BehaviorSubject } from 'rxjs'
import { bind } from '@react-rxjs/core'

const planPanelOpen$ = new BehaviorSubject<boolean>(false)
const sidebarOpen$ = new BehaviorSubject<boolean>(false)

export const [usePlanPanelOpen] = bind(planPanelOpen$, false)
/** Whether the sidebar is open on narrow screens (always shown on wide ones). */
export const [useSidebarOpen] = bind(sidebarOpen$, false)

export function openPlanPanel(): void {
  planPanelOpen$.next(true)
}

export function closePlanPanel(): void {
  planPanelOpen$.next(false)
}

export function toggleSidebar(): void {
  sidebarOpen$.next(!sidebarOpen$.getValue())
}

export function closeSidebar(): void {
  sidebarOpen$.next(false)
}
