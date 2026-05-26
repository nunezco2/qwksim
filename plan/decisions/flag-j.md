# FLAG-J — shared-constraint rivalry vs. explicit rivalry agent

**Status:** Closed (committed to **shared-constraint inside
resource agents' utility functions**) on 2026-05-22 at the T2.6
milestone.

## Decision

The bargaining utility's rivalry term (FLAG-C's
`γ · (1 − rivalry)` slot) is **read from a shared
[`ResourceContentionView`]** assembled at every bargaining round
from the live state of each fluid pool — currently the intra-site
network ([`FluidBandwidthPool`]) and the scratch I/O pool
([`ScratchIoPool`]).

The alternative — promoting "network rivalry" to its own
agent that participates in the Nash product population — is
**rejected**.

## Rationale

1. **Bargaining-population size matters for solver cost.** Phase 4
   will run a bounded best-response loop with `n = 3 + k`
   bargaining agents per super-site (`k` = QPUs). Adding two
   more agents (network rivalry, scratch I/O rivalry) raises the
   inner-loop cost by `≈ 2 / (3 + k)`. At the headline `k = 2`
   that is a ~40 % cost hit, with no corresponding modelling win.

2. **Rivalry is a scalar pressure, not a contested allocation.**
   A network rivalry "agent" would have no allocation to give or
   take — its only output would be a scalar consumed by every
   other utility. That's exactly what a *value type* is for.
   Promoting it to an agent multiplies the agent count without
   adding decision authority.

3. **Composes cleanly with FLAG-C.** The
   [`ResourceContentionView::total_rivalry`] aggregate is a
   single number in `[0, 1]` that drops into the FLAG-C utility
   formula `α·deadline_slack + β·realised_fidelity +
   γ·(1 − rivalry) − δ·degradation_penalty` without re-shaping
   the bargaining solver's Nash-product evaluator.

4. **Future contributors compose by extension.** When NUMA
   memory bandwidth or QPU calibration drift acquires a rivalry
   scalar of its own, it lands on `ResourceContentionView` as
   another field and feeds the same noisy-OR aggregate. No
   agent-population churn.

## Aggregation rule

`total_rivalry = 1 − ∏ (1 − r_i)` — **noisy-OR** across every
fluid pool's `rivalry()`. Properties (all unit-tested in
`crates/qwksim-resources/src/contention.rs`):

- Range: `total_rivalry ∈ [0, 1]` whenever every `r_i ∈ [0, 1]`.
- Monotonicity: increasing any `r_i` weakly increases the total.
- Saturation: if any `r_i == 1` the total is `1`; if every
  `r_i == 0` the total is `0`.
- Commutativity: builder-call order does not affect the result.

The choice between noisy-OR, `max(r_i)`, and a weighted sum is
not load-bearing for the headline metrics — the cooperative
bargaining solver only requires the total to be monotonic and
bounded in `[0, 1]`. Noisy-OR was chosen for its asymmetric
saturation (any single resource going saturated dominates) and
its differentiability (helps if a future PR replaces the
bounded-round best-response with a gradient-style solver).

## Re-opening conditions

Reassess only if:

- The bargaining solver in Phase 4 measurably gains from giving
  the rivalry term decision authority of its own (e.g. it can
  *trade* rivalry across resources, which it cannot under the
  shared-constraint formulation).
- A reviewer requests a per-link rivalry that participates in
  the Nash product directly — at which point the network agent
  promotes to a full `ResourceAgent` impl with its own
  `accept`/`release`/`utility` and the
  `ResourceContentionView` becomes a degenerate one-pool view
  for legacy compatibility.

The escape ladder from §14.6 of `plan/solution_plan.md` does
**not** cover this decision (the ladder is about per-run
wall-clock budgets, not bargaining-population shape).

## Cross-references

- Solution plan: §2.2 (resource models), §2.5 (bargaining), the
  FLAG-J entry in the flag-closure ledger.
- T2.3 (#29) — `FluidBandwidthPool` and the `rivalry()` scalar
  this view aggregates.
- T2.4 (#30) — `ScratchIoPool` (second contributor).
- T2.6 (this PR) — `ResourceContentionView` and its monotonicity
  unit tests.
- FLAG-I (`plan/decisions/flag-i.md`) — the fluid linear-share
  decision that gives every contributor a scalar to emit in the
  first place.
