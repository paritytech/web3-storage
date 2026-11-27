// SPDX-License-Identifier: GPL-3.0-only
//
// Library State — the library's storage plan. A library exists once its plan
// has bytes: the host opens the bucket on the first `setPlan`, picks the
// provider, and asks the user to approve the price. Growing or extending the
// plan is another `setPlan` with the new totals. `quotePlan` prices a target
// first, and `setPlan` for that target then charges at most the quoted price.

import { BehaviorSubject } from 'rxjs'
import { bind } from '@react-rxjs/core'
import type { RemoteStoragePlan, RemoteStorageQuote } from '@parity/truapi'
import { getPlan, quotePlan as quoteRemotePlan, setPlan, RemoteStorageError } from '@/lib/remote-library'

const DAY_SECONDS = 24n * 60n * 60n

/**
 * The largest `u128`: a quote limit that every price meets, so a quote shows
 * any price. `setPlan` enforces the user's limit with the quoted price.
 */
const ANY_PRICE = (1n << 128n) - 1n

export type PlanRequestStage = 'idle' | 'requesting' | 'failed'

export interface PlanRequestState {
  stage: PlanRequestStage
  /** Why the last request failed. */
  message?: string
  /** The host's price when it was above the most the user would pay. */
  quoted?: bigint
}

/** A plan photos asks for: a total size, funded until a time. */
export interface PlanTarget {
  /** Total bytes the library may hold. */
  bytes: bigint
  /** When the plan stays funded until, in seconds since the Unix epoch. */
  fundedUntil: bigint
}

/** A target's quote, or why it could not be quoted. */
export type PlanQuote =
  | { ok: true; target: PlanTarget; quote: RemoteStorageQuote }
  | { ok: false; target: PlanTarget; message: string }

/**
 * The target of a plan of `bytes` funded for `days` more days: from the end of
 * `plan` if it is still funded, otherwise from now. The host treats
 * `fundedUntil` as the exact end, so counting from now would shorten a plan.
 */
export function planTarget(bytes: bigint, days: number, plan: RemoteStoragePlan | null): PlanTarget {
  const now = BigInt(Math.floor(Date.now() / 1000))
  const start = plan !== null && !hasLapsed(plan) && plan.fundedUntil > now ? plan.fundedUntil : now
  return { bytes, fundedUntil: start + BigInt(days) * DAY_SECONDS }
}

// ─────────────────────────────────────────────────────────────────────────────
// State
// ─────────────────────────────────────────────────────────────────────────────

const plan$ = new BehaviorSubject<RemoteStoragePlan | null>(null)
const planError$ = new BehaviorSubject<string | undefined>(undefined)
const planRequest$ = new BehaviorSubject<PlanRequestState>({ stage: 'idle' })

export const [usePlan] = bind(plan$, null)
export const [usePlanError] = bind(planError$, undefined)
export const [usePlanRequest] = bind(planRequest$, { stage: 'idle' })

/** Whether `plan` describes a library: one with room for files. */
export function hasLibrary(plan: RemoteStoragePlan | null): boolean {
  return plan !== null && plan.bytes > 0n
}

/**
 * Whether `plan`'s library has lapsed: its funding ended without renewal, so
 * the backend no longer keeps the files, and the host rejects reads.
 */
export function hasLapsed(plan: RemoteStoragePlan | null): boolean {
  return plan?.state === 'Lapsed'
}

// ─────────────────────────────────────────────────────────────────────────────
// Actions
// ─────────────────────────────────────────────────────────────────────────────

/** Read the library's plan. */
export async function loadPlan(): Promise<void> {
  planError$.next(undefined)
  try {
    plan$.next(await getPlan())
  } catch (err) {
    planError$.next(err instanceof Error ? err.message : 'Could not read the storage plan.')
  }
}

/** Price `target` without asking the user or paying. */
export async function quotePlan(target: PlanTarget): Promise<PlanQuote> {
  try {
    return { ok: true, target, quote: await quoteRemotePlan(target.bytes, target.fundedUntil, ANY_PRICE) }
  } catch (err) {
    return { ok: false, target, message: failure(err).message ?? 'Could not price this plan.' }
  }
}

/**
 * Ask the user to pay for `target`, at most `maxPrice`, normally the price
 * `quotePlan` gave for the same target. Returns whether the plan was set.
 */
export async function requestPlan(target: PlanTarget, maxPrice: bigint): Promise<boolean> {
  planRequest$.next({ stage: 'requesting' })
  try {
    const { plan } = await setPlan(target.bytes, target.fundedUntil, maxPrice)
    plan$.next(plan)
    planRequest$.next({ stage: 'idle' })
    return true
  } catch (err) {
    planRequest$.next(failure(err))
    return false
  }
}

export function resetPlanRequest(): void {
  planRequest$.next({ stage: 'idle' })
}

function failure(err: unknown): PlanRequestState {
  if (!(err instanceof RemoteStorageError)) {
    return { stage: 'failed', message: err instanceof Error ? err.message : 'The plan request failed.' }
  }
  switch (err.code) {
    case 'PriceExceeded':
      return { stage: 'failed', message: 'The price changed since it was shown.', quoted: err.quoted }
    case 'Declined':
      return { stage: 'failed', message: 'You declined the payment.' }
    case 'InsufficientFunds':
      return { stage: 'failed', message: 'The balance does not cover the price.' }
    case 'NotAvailable':
      return { stage: 'failed', message: 'No storage provider offers this plan.' }
    case 'BelowUsage':
      return { stage: 'failed', message: 'The library already holds more than that.' }
    default:
      return { stage: 'failed', message: err.message }
  }
}
