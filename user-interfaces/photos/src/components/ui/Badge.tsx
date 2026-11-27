// SPDX-License-Identifier: GPL-3.0-only

import { cva, type VariantProps } from 'class-variance-authority'
import { cn } from '@/utils/cn'

const badgeVariants = cva(
  'inline-flex items-center rounded-full px-2 py-1 text-xs font-medium',
  {
    variants: {
      variant: {
        default: 'bg-action-secondary text-primary',
        secondary: 'bg-surface-nested text-secondary',
        success: 'bg-surface-nested text-success',
        warning: 'bg-surface-nested text-warning',
        destructive: 'bg-surface-nested text-error',
        outline: 'border text-secondary',
      },
    },
    defaultVariants: {
      variant: 'default',
    },
  }
)

export interface BadgeProps
  extends React.HTMLAttributes<HTMLDivElement>,
    VariantProps<typeof badgeVariants> {}

function Badge({ className, variant, ...props }: BadgeProps) {
  return <div className={cn(badgeVariants({ variant }), className)} {...props} />
}

export { Badge, badgeVariants }
