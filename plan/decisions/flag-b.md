# FLAG-B — Disagreement-point evaluator

**Status:** Closed (committed to **FCFS-on-stale-advertised-summaries,
evaluated through the same FLAG-C utility function**) on
2026-06-08 at the T4.3 milestone.

## Decision

The Nash-bargaining disagreement point `d_i(view)` for every
participating agent is the utility that agent would realise
under a **FIFO admission projected against the same advertised
state** the bargaining round sees, scored by the **same
[`UtilityFn::evaluate`] formula** (FLAG-C closed; see §6.3 of
`plan/solution_plan.md` and `crates/qwksim-bargain/src/utility.rs`).
Concretely:

```rust
impl UtilityFn for CategoryWeights {
    fn disagreement(&self, terms_under_fcfs: &UtilityTerms) -> f64 {
        self.evaluate(terms_under_fcfs)
    }
}
```

The upstream runner (T4.4+) is responsible for projecting the
FCFS-on-stale [`UtilityTerms`] — *what would the workflow's
deadline / fidelity / rivalry / degradation look like if FCFS
on the advertised state had admitted it instead of the
bargain?* — and feeding the result into `disagreement`. The
trait method itself is therefore a thin re-evaluation of the
shared formula.

### Rejected alternatives

- **`d_i = 0`** (the trivial floor). Makes every bargain look
  feasible vacuously and breaks individual-rationality
  guarantees: agents whose bargained allocation lies below the
  FCFS baseline would still accept. The whole point of a
  disagreement point is to *floor* the bargain at the agent's
  outside option.

- **Bargain-and-recompute** (run the bargaining solver, take
  the result as the disagreement). Circular: the disagreement
  point feeds the Nash product the bargain itself maximises.
  Unworkable.

- **Oracular FCFS** (project FCFS against the *true* state
  rather than the advertised state). Information-regime
  inconsistent — the bargainers see the advertised state, so
  comparing against an outcome only an oracle could achieve
  would over-credit bargaining whenever the advertised state
  is stale. We need the disagreement and the bargain to share
  the same information regime, otherwise the Pareto-dominance
  test we use for the cooperative bargain validity test
  (`bargained_utility ≥ disagreement_utility`) is biased.

- **Per-agent threat point from a separate utility function**
  (e.g. an "egoist" version). Adds a tuning surface and a
  formula divergence between bargained and disagreement
  outcomes. The simpler choice is to share the formula and
  let the *terms* carry the difference.

## Rationale

1. **Information-regime consistency.** Both the bargain and
   the disagreement see the τ_adv-stale advertised summaries,
   so the Pareto comparison `bargained ≥ disagreement` reflects
   the value of the bargaining mechanism rather than the
   information gap between FCFS and an oracle.

2. **Single-function audit surface.** The FLAG-C formula
   (§6.3) is the *only* place coefficients live for both
   bargained and disagreement payoffs. A coefficient sweep
   (`α / β / γ / δ` sensitivity) shifts both simultaneously,
   keeping the relative comparison meaningful.

3. **Cheap-to-evaluate.** `disagreement` is a single
   coefficient-wise affine combination — the same cost as
   `evaluate`. The bargaining inner loop calls it once per
   round per agent; this is negligible against the per-round
   Nash-product cost.

4. **Pareto-dominance test stays clean.** The Nash bargaining
   theorem guarantees the bargained solution is individually
   rational, i.e. `u_i ≥ d_i` for every participating agent,
   when both quantities come from the same formula. The
   `bargained ≥ disagreement` invariant lands in the T4.8
   proptest; T4.3 pins the *degenerate* case (single resource,
   single workflow) where `bargained = disagreement` is the
   tightest the inequality can be.

5. **FCFS-on-stale is the realistic outside option.** If the
   cooperative bargain fails, the workload defaults to a FIFO
   queue against the advertised state — that is the actual
   fallback path in the simulator, not an oracle baseline.
   Modelling the disagreement as the *real* alternative keeps
   the gap meaningful.

## Implementation status

- `UtilityFn::disagreement` default implementation delegates
  to `Self::evaluate` in `crates/qwksim-bargain/src/utility.rs`
  (T4.3).
- The FCFS-on-stale **terms** projection lives in the
  experiment runner and lands at T4.4 alongside the
  `projected_completion` / `realised_fidelity` /
  `resource_contention_rivalry` / `degradation_penalty`
  helpers. Today the T4.3 tests pass pre-computed
  [`UtilityTerms`] directly so the disagreement-dominance
  contract is pinnable without waiting on T4.4.
- T4.8 proptest will sweep this invariant across randomised
  bargaining rounds.

## References

- §6.4 of `plan/solution_plan.md` — closed-form definition.
- §13.3 task list — T4.3 issue #49, T4.4 issue #50, T4.8 issue
  #54.
- §2.5 of `plan/solution_plan.md` — `UtilityFn` trait shape.
