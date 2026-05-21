//! Cooperative Nash-bargaining solver for `qwksim`.
//!
//! Implements the bargaining inner loop at the heart of every
//! distributed scheduling decision: the `NashProductEvaluator` (sums
//! log utilities for numerical stability), the `BargainingRound`
//! best-response driver with an `R_max` budget, the `UtilityFn` trait
//! parameterised over the FLAG-C utility formula `u = α·deadline_slack
//! ÷ deadline + β·realised_fidelity + γ·(1 − rivalry) −
//! δ·degradation_penalty` (per-category α/β/γ/δ; category 5 has
//! β = δ = 0), and the disagreement-point evaluator returning each
//! agent's payoff under the FCFS-on-stale-advertised-summaries
//! baseline (FLAG-B closed).
//!
//! Entry point: `solve_bargain(workflow, view) -> BargainingOutcome`.
//!
//! See `plan/solution_plan.md` §2.5 and §6 for the solver
//! architecture and proptest invariants.
