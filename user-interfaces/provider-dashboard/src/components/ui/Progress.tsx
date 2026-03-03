import { cn } from '@/utils/cn'

interface ProgressProps {
  value: number
  max?: number
  className?: string
  indicatorClassName?: string
}

export function Progress({ value, max = 100, className, indicatorClassName }: ProgressProps) {
  const percentage = Math.min(Math.max((value / max) * 100, 0), 100)

  return (
    <div className={cn('relative h-2 w-full overflow-hidden rounded-full bg-gray-800', className)}>
      <div
        className={cn(
          'h-full transition-all duration-300',
          percentage >= 90
            ? 'bg-red-500'
            : percentage >= 70
            ? 'bg-yellow-500'
            : 'bg-purple-500',
          indicatorClassName
        )}
        style={{ width: `${percentage}%` }}
      />
    </div>
  )
}
