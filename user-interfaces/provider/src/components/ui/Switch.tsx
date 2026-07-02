// SPDX-License-Identifier: Apache-2.0

import { forwardRef } from 'react'
import { cn } from '@/utils/cn'

export interface SwitchProps extends Omit<React.InputHTMLAttributes<HTMLInputElement>, 'type'> {
  onCheckedChange?: (checked: boolean) => void
}

const Switch = forwardRef<HTMLInputElement, SwitchProps>(
  ({ className, checked, onCheckedChange, onChange, ...props }, ref) => {
    const handleChange = (e: React.ChangeEvent<HTMLInputElement>) => {
      onChange?.(e)
      onCheckedChange?.(e.target.checked)
    }

    return (
      <label className={cn('relative inline-flex cursor-pointer items-center', className)}>
        <input
          type="checkbox"
          className="peer sr-only"
          checked={checked}
          onChange={handleChange}
          ref={ref}
          {...props}
        />
        <div
          className={cn(
            'h-6 w-11 rounded-full bg-gray-700 transition-colors',
            'after:absolute after:left-[2px] after:top-[2px] after:h-5 after:w-5 after:rounded-full after:bg-white after:transition-all after:content-[""]',
            'peer-checked:bg-purple-600 peer-checked:after:translate-x-full',
            'peer-focus:ring-2 peer-focus:ring-purple-500 peer-focus:ring-offset-2 peer-focus:ring-offset-gray-900',
            'peer-disabled:cursor-not-allowed peer-disabled:opacity-50'
          )}
        />
      </label>
    )
  }
)
Switch.displayName = 'Switch'

export { Switch }
