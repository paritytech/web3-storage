// SPDX-License-Identifier: GPL-3.0-only

import { Search } from 'lucide-react'
import { Input } from '@/components/ui'

interface SearchInputProps {
  value: string
  onChange: (value: string) => void
  placeholder: string
  /** Rendered as `data-testid="{area}-search"`. */
  area: string
}

export function SearchInput({ value, onChange, placeholder, area }: SearchInputProps) {
  return (
    <div className="relative max-w-md">
      <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-gray-500" />
      <Input
        className="pl-9"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        data-testid={`${area}-search`}
      />
    </div>
  )
}
