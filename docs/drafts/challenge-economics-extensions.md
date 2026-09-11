# Challenge Economics — Extensions (Draft)

> **Draft — needs triage.** Speculative extension extracted from the canonical
> design's "Future Directions". **Not implemented**, and it presupposes a base
> mechanism that is *also* not implemented yet — see the gap note below.

## Capped Split for the General Public

Today the general public gets no cost split (challengers pay the response in
full) to close the DDoS hole. A capped version could give the public *some*
leverage without reopening that hole: apply the split to anonymous challenges
too, but only up to a per-provider budget over a rolling window of X blocks.
Once the budget is spent, further public challenges revert to full pay until the
window resets. The budget caps the total a crowd can extract per window, so the
DDoS attack is bounded rather than open-ended. Left out of the initial design;
addable later without changing the core mechanism.

## ⚠️ Prerequisite gap: the two-tier split isn't implemented

Capped Split refines the **authorized-vs-public** distinction — but that
distinction does not exist in code today:

- The design specifies two tiers (authorized bucket members/owners get a cost
  split where the provider bears a fraction; the general public pays in full,
  for anti-DoS).
- The pallet (`crates/pallets/storage-provider/src/lib.rs`, challenge
  resolution) applies a purely **response-time-based** split
  (challenger 90→50%) to **every** challenger, with no authorized/public check
  anywhere. So the design's anti-DoS premise is currently unenforced, and a
  public challenger already receives the split.
- Possible further divergence to verify: on a *valid* defense the code slashes
  the provider's stake for `provider_cost`, whereas the design states a valid
  response never touches stake (the fraction should come from the challenger's
  deposit).

**Triage:** decide the challenge cost model end-to-end — implement the two tiers
(then Capped Split becomes a coherent add-on) or amend the design to match the
flat time-based split that shipped. Capped Split should not be revisited until
the base tier is settled.
