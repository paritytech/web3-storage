// SPDX-License-Identifier: Apache-2.0

/**
 * UI settings that persist across reloads (preferences, not cached chain data).
 *
 * Currently holds the auto-refresh interval used to periodically re-fetch
 * provider data. `0` means auto-refresh is off.
 */

import { BehaviorSubject } from 'rxjs'
import { bind } from '@react-rxjs/core'

const AUTO_REFRESH_KEY = 'provider-auto-refresh-secs'

/** Selectable intervals in seconds; `0` is "Off". */
export const AUTO_REFRESH_OPTIONS = [0, 15, 30, 60] as const
export type AutoRefreshSecs = (typeof AUTO_REFRESH_OPTIONS)[number]

/** Default: refresh every 30s. */
export const DEFAULT_AUTO_REFRESH_SECS: AutoRefreshSecs = 30

function isValidInterval(n: number): n is AutoRefreshSecs {
  return (AUTO_REFRESH_OPTIONS as readonly number[]).includes(n)
}

function loadAutoRefreshSecs(): AutoRefreshSecs {
  try {
    const raw = localStorage.getItem(AUTO_REFRESH_KEY)
    if (raw === null) return DEFAULT_AUTO_REFRESH_SECS
    const n = Number(raw)
    return isValidInterval(n) ? n : DEFAULT_AUTO_REFRESH_SECS
  } catch {
    return DEFAULT_AUTO_REFRESH_SECS
  }
}

const autoRefreshSecs$ = new BehaviorSubject<AutoRefreshSecs>(loadAutoRefreshSecs())

export const [useAutoRefreshSecs] = bind(autoRefreshSecs$, DEFAULT_AUTO_REFRESH_SECS)

/** Update the auto-refresh interval and persist the choice. */
export function setAutoRefreshSecs(secs: AutoRefreshSecs): void {
  autoRefreshSecs$.next(secs)
  try {
    localStorage.setItem(AUTO_REFRESH_KEY, String(secs))
  } catch {
    /* ignore */
  }
}

/** Human-readable label for an interval. */
export function formatInterval(secs: AutoRefreshSecs): string {
  return secs === 0 ? 'Off' : `${secs}s`
}
