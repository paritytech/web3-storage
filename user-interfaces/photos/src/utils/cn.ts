// SPDX-License-Identifier: GPL-3.0-only

import { clsx, type ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'

/**
 * Merge Tailwind class names, resolving conflicts. `clsx` flattens conditional
 * inputs into a class string, then `twMerge` dedupes clashing Tailwind utilities
 * so the last one wins (e.g. `cn('px-2', 'px-4')` → `px-4`).
 */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}
