// SPDX-License-Identifier: GPL-3.0-only
//
// The storage plans photos offers: a size per tier and a billing period. The
// host picks the provider and quotes each tier's price.

export type Billing = 'monthly' | 'yearly'

/** How many days one payment funds the library for. */
export const BILLING_DAYS: Record<Billing, number> = { monthly: 30, yearly: 365 }

export interface PlanTier {
  id: string
  name: string
  /** How much the library may hold. */
  bytes: bigint
}

const GIB = 1024n ** 3n

export const PLAN_TIERS: PlanTier[] = [
  { id: 'basic', name: 'Basic', bytes: 1n * GIB },
  { id: 'standard', name: 'Standard', bytes: 10n * GIB },
  { id: 'premium', name: 'Premium', bytes: 100n * GIB },
]

/** The tier whose size matches the plan's, if any. */
export function currentTier(planBytes: bigint): PlanTier | undefined {
  return PLAN_TIERS.find((tier) => tier.bytes === planBytes)
}
