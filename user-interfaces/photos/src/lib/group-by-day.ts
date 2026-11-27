// SPDX-License-Identifier: GPL-3.0-only
//
// Group photos by the local calendar day they were stored, for the grid's
// date headings.

export interface DayGroup<T> {
  /** Local midnight of the day, in milliseconds since the Unix epoch. */
  day: number
  items: T[]
}

/** Group `items` by local day, keeping their order; `items` should be newest first. */
export function groupByDay<T extends { modified: number }>(items: T[]): DayGroup<T>[] {
  const groups: DayGroup<T>[] = []
  for (const item of items) {
    const day = startOfDay(item.modified)
    const last = groups.at(-1)
    if (last && last.day === day) last.items.push(item)
    else groups.push({ day, items: [item] })
  }
  return groups
}

/** The heading for a day: "Today", "Yesterday", or a date like "Mon, 12 Nov 2026". */
export function dayLabel(day: number, now: number = Date.now()): string {
  const today = startOfDay(now)
  if (day === today) return 'Today'
  if (day === startOfDay(today - 1)) return 'Yesterday'
  const date = new Date(day)
  const sameYear = date.getFullYear() === new Date(now).getFullYear()
  return date.toLocaleDateString(undefined, {
    weekday: 'short',
    day: 'numeric',
    month: 'short',
    year: sameYear ? undefined : 'numeric',
  })
}

function startOfDay(time: number): number {
  const date = new Date(time)
  date.setHours(0, 0, 0, 0)
  return date.getTime()
}
