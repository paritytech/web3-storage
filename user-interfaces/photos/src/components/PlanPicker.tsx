// SPDX-License-Identifier: GPL-3.0-only
//
// The storage plans: a monthly/yearly switch and one card per tier. Each card
// shows the price the host quotes for that size, funded for one more period:
// from the end of the current plan if it is funded, otherwise from now.
// Choosing a tier asks the host for that plan at the quoted price; the host
// picks the provider and asks the user to approve it.

import { useEffect, useState } from 'react'
import { AlertTriangle, Check } from 'lucide-react'
import type { RemoteStoragePlan } from '@parity/truapi'
import {
  hasLapsed,
  hasLibrary,
  planTarget,
  quotePlan,
  requestPlan,
  resetPlanRequest,
  usePlanRequest,
  type PlanQuote,
} from '@/state/library.state'
import { BILLING_DAYS, PLAN_TIERS, currentTier, type Billing, type PlanTier } from '@/lib/plans'
import { Button } from '@/components/ui/Button'
import { Spinner } from '@/components/ui/Spinner'
import { formatAmount, formatBytesBinary, getTokenSymbol } from '@web3-storage/format'
import { cn } from '@/utils/cn'

export function PlanPicker({ plan, onDone }: { plan: RemoteStoragePlan; onDone?: () => void }) {
  const request = usePlanRequest()
  const [billing, setBilling] = useState<Billing>('monthly')
  const [chosen, setChosen] = useState<PlanTier | undefined>(undefined)
  const active = hasLibrary(plan) && !hasLapsed(plan)
  const current = active ? currentTier(plan.bytes) : undefined
  const requesting = request.stage === 'requesting'
  const quotes = useTierQuotes(billing, plan)

  async function choose(tier: PlanTier, maxPrice?: bigint) {
    const quoted = quotes[tier.id]
    if (!quoted?.ok) return
    setChosen(tier)
    if (await requestPlan(quoted.target, maxPrice ?? quoted.quote.price)) onDone?.()
  }

  return (
    <div className="space-y-4">
      <div className="inline-flex rounded-full bg-surface-nested p-1" role="radiogroup" aria-label="Billing period">
        {(['monthly', 'yearly'] as const).map((period) => (
          <button
            key={period}
            type="button"
            role="radio"
            aria-checked={billing === period}
            disabled={requesting}
            onClick={() => setBilling(period)}
            className={cn(
              'cursor-pointer rounded-full px-4 py-1.5 text-sm font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50',
              billing === period
                ? 'bg-action-secondary text-primary'
                : 'text-secondary hover:text-secondary-hover',
            )}
            data-testid={`plan-billing-${period}`}
          >
            {period === 'monthly' ? 'Monthly' : 'Yearly'}
          </button>
        ))}
      </div>

      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        {PLAN_TIERS.map((tier) => {
          const isCurrent = current?.id === tier.id
          const tooSmall = active && tier.bytes < plan.usedBytes
          const busy = requesting && chosen?.id === tier.id
          const quoted = quotes[tier.id]
          const unpriced = !quoted?.ok || quoted.quote.startsAt !== undefined
          return (
            <div
              key={tier.id}
              className={cn(
                'flex flex-col rounded-nested bg-surface-nested p-4',
                isCurrent && 'outline outline-1 outline-indicator',
              )}
              data-testid={`plan-tier-${tier.id}`}
            >
              <div className="flex items-center justify-between">
                <h3 className="font-semibold leading-tight text-primary">{tier.name}</h3>
                {isCurrent && <span className="text-xs text-secondary">Current</span>}
              </div>
              <p className="mt-3 text-2xl font-semibold leading-tight text-primary">{formatBytesBinary(tier.bytes)}</p>
              <TierPrice quoted={quoted} billing={billing} extendsPlan={active} />
              <Button
                className="mt-4"
                size="sm"
                variant={isCurrent ? 'secondary' : 'default'}
                disabled={requesting || tooSmall || unpriced}
                title={tooSmall ? 'Your library already holds more than this.' : undefined}
                onClick={() => void choose(tier)}
                data-testid={`plan-choose-${tier.id}`}
              >
                {busy ? (
                  <>
                    <Spinner size="sm" className="mr-2" /> Waiting for approval
                  </>
                ) : isCurrent ? (
                  `Renew for a ${billing === 'monthly' ? 'month' : 'year'}`
                ) : (
                  `Choose ${tier.name}`
                )}
              </Button>
            </div>
          )
        })}
      </div>

      {active && (
        <p className="text-xs text-tertiary">
          Your plan is paid until {new Date(Number(plan.fundedUntil) * 1000).toLocaleDateString()}. A new plan
          does not end before that date.
        </p>
      )}

      {request.stage === 'failed' && (
        <div className="space-y-3 rounded-nested bg-surface-nested p-3 text-sm" role="alert">
          <div className="flex items-start gap-2 text-error">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
            <span data-testid="plan-error">
              {request.message}
              {request.quoted !== undefined && (
                <>
                  {' '}
                  Your host now quotes {formatAmount(request.quoted)} {getTokenSymbol()}.
                </>
              )}
            </span>
          </div>
          <div className="flex items-center gap-2">
            {request.quoted !== undefined && chosen && (
              <Button size="sm" onClick={() => void choose(chosen, request.quoted)}>
                <Check className="mr-1.5 h-4 w-4" /> Pay {formatAmount(request.quoted)} {getTokenSymbol()}
              </Button>
            )}
            <Button size="sm" variant="ghost" onClick={() => resetPlanRequest()}>
              Dismiss
            </Button>
          </div>
        </div>
      )}
    </div>
  )
}

/** The longest delay `setTimeout` accepts; it treats a longer one as 0. */
const MAX_TIMEOUT = 2 ** 31 - 1

/**
 * Each tier's quote for one more `billing` period (see `planTarget`), priced
 * again when a quote expires or the plan changes. A tier is missing while its
 * quote is pending.
 */
function useTierQuotes(billing: Billing, plan: RemoteStoragePlan): Record<string, PlanQuote> {
  const [quotes, setQuotes] = useState<Record<string, PlanQuote>>({})
  useEffect(() => {
    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const refresh = async () => {
      setQuotes({})
      const results = await Promise.all(
        PLAN_TIERS.map((tier) => quotePlan(planTarget(tier.bytes, BILLING_DAYS[billing], plan))),
      )
      if (cancelled) return
      setQuotes(Object.fromEntries(PLAN_TIERS.map((tier, index) => [tier.id, results[index]])))
      const expiries = results.flatMap((result) => (result.ok ? [Number(result.quote.validUntil) * 1000] : []))
      if (expiries.length > 0) {
        const delay = Math.max(1_000, Math.min(...expiries) - Date.now())
        timer = setTimeout(() => void refresh(), Math.min(MAX_TIMEOUT, delay))
      }
    }
    void refresh()
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
  }, [billing, plan.bytes, plan.fundedUntil])
  return quotes
}

/**
 * A tier's quoted price, or why it has none. When the target extends a funded
 * plan, the price can cover more than one period, so it shows the end date.
 */
function TierPrice({
  quoted,
  billing,
  extendsPlan,
}: {
  quoted: PlanQuote | undefined
  billing: Billing
  extendsPlan: boolean
}) {
  if (!quoted) {
    return (
      <p className="mt-1 flex items-center gap-2 text-sm text-secondary">
        <Spinner size="sm" /> Getting the price…
      </p>
    )
  }
  if (!quoted.ok) {
    return (
      <p className="mt-1 text-sm text-error" data-testid="plan-quote-error">
        {quoted.message}
      </p>
    )
  }
  if (quoted.quote.startsAt !== undefined) {
    return (
      <p className="mt-1 text-sm text-secondary">
        Smaller than your plan, so it would start when your plan ends. Photos cannot do that yet.
      </p>
    )
  }
  const price = `${formatAmount(quoted.quote.price)} ${getTokenSymbol()}`
  return (
    <p className="mt-1 text-sm text-secondary" data-testid="plan-price">
      {extendsPlan
        ? `${price} until ${new Date(Number(quoted.target.fundedUntil) * 1000).toLocaleDateString()}`
        : `${price} per ${billing === 'monthly' ? 'month' : 'year'}`}
    </p>
  )
}
