// SPDX-License-Identifier: GPL-3.0-only

import { Card, CardContent } from '@/components/ui'

interface StatTileProps {
  label: string
  /** `null` renders the loading placeholder; a loaded zero must be passed as "0". */
  value: string | null
  testId: string
  sub?: string
}

export function StatTile({ label, value, testId, sub }: StatTileProps) {
  return (
    <Card>
      <CardContent className="p-6">
        <p className="text-sm text-gray-400">{label}</p>
        <p className="mt-1 text-2xl font-semibold text-gray-100" data-testid={testId}>
          {value ?? '…'}
        </p>
        {sub && <p className="mt-1 text-xs text-gray-500">{sub}</p>}
      </CardContent>
    </Card>
  )
}
