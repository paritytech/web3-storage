// SPDX-License-Identifier: Apache-2.0

/**
 * Stable, globally-unique identity for a challenge.
 *
 * On chain a challenge is addressed by `{ deadline, index }` and the UI's `id`
 * holds the per-deadline `index`, so `id` alone collides across deadline blocks
 * (e.g. two challenges both at index 0). Anything that identifies a single
 * challenge — cache keys, React list keys, in-flight/error tracking — must key
 * on this rather than on `id`.
 *
 * Kept in its own leaf module (no side-effecting imports) so it can be unit
 * tested without pulling in the chain/state graph.
 */
export function challengeKey(c: { deadline: number; id: number }): string {
  return `${c.deadline}-${c.id}`
}
