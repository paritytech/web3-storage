// SPDX-License-Identifier: GPL-3.0-only

import { CloudOff } from 'lucide-react'
import { Card } from '@/components/ui'

/**
 * Rendered instead of a list's empty state when the section's query failed —
 * "we couldn't read it" must never masquerade as "the chain holds nothing".
 */
export function SectionUnavailable({ section }: { section: string }) {
  return (
    <Card
      className="flex flex-col items-center gap-2 border-yellow-900 py-16 text-center"
      data-testid={`${section}-unavailable`}
    >
      <CloudOff className="h-8 w-8 text-yellow-600" />
      <p className="text-yellow-300">Could not load {section} from this network</p>
      <p className="text-xs text-gray-500">
        The storage item may not exist on this runtime, or the query failed. Try refreshing.
      </p>
    </Card>
  )
}
