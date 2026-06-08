//! FLAG-C closed-form utility + per-category weights (T4.2).
//!
//! Per §6.3 of the engineering plan, each agent's per-workflow
//! utility is the affine combination
//!
//! ```text
//! u = α · deadline_term
//!   + β · fidelity_term
//!   + γ · rivalry_term
//!   − δ · degradation_penalty
//! ```
//!
//! where
//!
//! - `deadline_term = max(deadline − projected_completion, 0) /
//!   deadline ∈ [0, 1]`
//! - `fidelity_term = realised_fidelity ∈ [0, 1]` (sourced from
//!   `qwksim_qpu::QpuAgent::fidelity_term_at` at T3.7),
//! - `rivalry_term = 1 − resource_contention_rivalry(alloc, view)
//!   ∈ [0, 1]`,
//! - `degradation_penalty ≥ 0` — workflow-specific.
//!
//! ## Scope of this PR
//!
//! T4.2 lands the **closed-form arithmetic** and the
//! `CategoryWeights` (α/β/γ/δ) configuration surface.
//! Computing the four [`UtilityTerms`] from `(alloc, view,
//! workflow)` requires the projected-completion / realised-
//! fidelity / contention-rivalry / degradation-penalty helpers
//! which are themselves Phase-4 wiring (T4.4 onwards). For
//! T4.2 the function takes the four terms as inputs and the
//! `UtilityFn` trait shape matches that boundary — callers
//! pass already-computed [`UtilityTerms`] in. The wiring layer
//! at T4.4 will compose them.
//!
//! ## Trait
//!
//! [`UtilityFn::evaluate`] is the per-agent surface the
//! best-response loop consumes. [`UtilityFn::disagreement`]
//! returns the FCFS-on-stale baseline payoff (FLAG-B closed,
//! T4.3) — the *same* utility function evaluated against the
//! [`UtilityTerms`] the workflow would have realised under a
//! simple FIFO admission projected against the same advertised
//! state the bargaining agents see. The default implementation
//! therefore simply delegates back to [`UtilityFn::evaluate`],
//! capturing the FLAG-B contract: the disagreement and the
//! bargained outcome share an information regime and share an
//! evaluation function — only the *terms* differ.
//!
//! Concretely the FCFS-on-stale projection lives upstream in
//! the experiment runner (T4.4+); this module is the
//! evaluation half. See [`plan/decisions/flag-b.md`] for the
//! design rationale.

use serde::Deserialize;

/// The four scalar terms that feed the FLAG-C closed-form
/// utility. Computed upstream from `(alloc, view, workflow)`
/// when the Phase-4 wiring lands; passed in pre-computed here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UtilityTerms {
    /// `max(deadline − projected_completion, 0) / deadline ∈
    /// [0, 1]` — fraction of the deadline left as slack.
    pub deadline_term: f64,
    /// Realised fidelity `∈ [0, 1]` (e.g. `QpuAgent::fidelity_term_at`).
    pub fidelity_term: f64,
    /// `1 − resource_contention_rivalry ∈ [0, 1]` — higher is
    /// less rivalry.
    pub rivalry_term: f64,
    /// Degradation penalty `≥ 0` (smaller is better).
    pub degradation: f64,
}

/// Per-category FLAG-C weights (α, β, γ, δ).
///
/// Loaded from per-category sections of the experiment TOML
/// manifest (`[categories.<name>.weights]`). Per §6.3
/// category 5 (`pure_classical`) is constrained to `β = 0` and
/// `δ = 0`; this invariant is **not** enforced by the
/// struct itself — the runner enforces it at manifest load
/// time — but the `default_for` catalogue below honours it.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct CategoryWeights {
    /// `α` — weight on deadline slack.
    pub alpha: f64,
    /// `β` — weight on realised fidelity.
    pub beta: f64,
    /// `γ` — weight on (1 − rivalry).
    pub gamma: f64,
    /// `δ` — weight on degradation penalty (subtracted).
    pub delta: f64,
}

/// Workflow category from §11.3. Snake-case TOML labels match
/// the experiment manifest's `[categories.<name>]` keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Variational quantum eigensolver.
    Vqe,
    /// Quantum approximate optimisation algorithm.
    Qaoa,
    /// Quantum machine learning.
    Qml,
    /// QC-Monte-Carlo / quantum phase estimation.
    QcMcQpe,
    /// Pure-classical workload (no QPU side). `β = δ = 0`.
    PureClassical,
    /// Tomography sweep.
    Tomography,
}

impl CategoryWeights {
    /// Parse the weights block from a TOML string. The TOML
    /// keys must be `alpha`, `beta`, `gamma`, `delta`.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Default weights per category (Q11.4 + Q12 envelopes).
    /// Category 5 (`PureClassical`) zeroes out `β` and `δ` per
    /// §6.3.
    ///
    /// These values are anchors for the headline scenario;
    /// the experiment TOML may override them per replicate.
    pub fn default_for(category: Category) -> Self {
        match category {
            Category::Vqe => Self {
                alpha: 0.30,
                beta: 0.50,
                gamma: 0.15,
                delta: 0.05,
            },
            Category::Qaoa => Self {
                alpha: 0.30,
                beta: 0.50,
                gamma: 0.15,
                delta: 0.05,
            },
            Category::Qml => Self {
                alpha: 0.25,
                beta: 0.55,
                gamma: 0.15,
                delta: 0.05,
            },
            Category::QcMcQpe => Self {
                alpha: 0.20,
                beta: 0.70,
                gamma: 0.05,
                delta: 0.05,
            },
            Category::PureClassical => Self {
                alpha: 0.70,
                beta: 0.00,
                gamma: 0.30,
                delta: 0.00,
            },
            Category::Tomography => Self {
                alpha: 0.20,
                beta: 0.65,
                gamma: 0.10,
                delta: 0.05,
            },
        }
    }
}

/// FLAG-C closed-form utility (§6.3):
///
/// ```text
/// u = α · deadline_term
///   + β · fidelity_term
///   + γ · rivalry_term
///   − δ · degradation_penalty
/// ```
///
/// Pure function — no allocation, no side effects, no
/// hidden mutable state. Each coefficient is independently
/// pinnable by zeroing the other three weights.
pub fn evaluate_utility(weights: &CategoryWeights, terms: &UtilityTerms) -> f64 {
    weights.alpha * terms.deadline_term
        + weights.beta * terms.fidelity_term
        + weights.gamma * terms.rivalry_term
        - weights.delta * terms.degradation
}

/// Per-agent utility-function trait. The bargaining solver
/// (T4.5) consumes the trait, not the concrete weights, so
/// future utility-function variants (e.g. T4.7 alternate
/// degradation models) drop in without changing the solver.
///
/// `disagreement` carries the FCFS-on-stale baseline payoff
/// (FLAG-B, T4.3). The default implementation delegates back
/// to [`Self::evaluate`] — the FLAG-B contract is that the
/// disagreement and the bargained outcome share an
/// information regime and share an evaluation function; only
/// the [`UtilityTerms`] differ. The upstream runner (T4.4+)
/// projects the FCFS-on-stale [`UtilityTerms`] and feeds them
/// in.
pub trait UtilityFn {
    /// Evaluate the utility at the given pre-computed
    /// [`UtilityTerms`]. Required.
    fn evaluate(&self, terms: &UtilityTerms) -> f64;

    /// Disagreement-point payoff under FCFS-on-stale (FLAG-B
    /// closed; see `plan/decisions/flag-b.md`).
    ///
    /// `terms_under_fcfs` is the [`UtilityTerms`] the workflow
    /// would have realised under a simple FIFO admission
    /// against the same advertised state. Default delegates
    /// back to [`Self::evaluate`] — the disagreement and the
    /// bargained outcome share an evaluation function, only
    /// the inputs differ.
    fn disagreement(&self, terms_under_fcfs: &UtilityTerms) -> f64 {
        self.evaluate(terms_under_fcfs)
    }
}

impl UtilityFn for CategoryWeights {
    fn evaluate(&self, terms: &UtilityTerms) -> f64 {
        evaluate_utility(self, terms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(deadline: f64, fidelity: f64, rivalry: f64, degradation: f64) -> UtilityTerms {
        UtilityTerms {
            deadline_term: deadline,
            fidelity_term: fidelity,
            rivalry_term: rivalry,
            degradation,
        }
    }

    fn weights(alpha: f64, beta: f64, gamma: f64, delta: f64) -> CategoryWeights {
        CategoryWeights {
            alpha,
            beta,
            gamma,
            delta,
        }
    }

    /// T4.2 acceptance gate: pin every coefficient against a
    /// hand-computed expectation. All four terms active, all
    /// four weights non-zero.
    #[test]
    fn evaluate_utility_pins_each_coefficient_against_hand_computed_value() {
        let w = weights(0.30, 0.50, 0.15, 0.05);
        let t = terms(0.80, 0.90, 0.70, 0.10);
        // u = 0.30·0.80 + 0.50·0.90 + 0.15·0.70 − 0.05·0.10
        //   = 0.24    + 0.45    + 0.105   − 0.005
        //   = 0.790
        let got = evaluate_utility(&w, &t);
        let want = 0.24 + 0.45 + 0.105 - 0.005;
        assert!(
            (got - want).abs() < 1e-15,
            "coefficient-by-coefficient pin failed: got {got}, want {want}",
        );
        assert!((got - 0.790).abs() < 1e-15, "expected 0.790, got {got}");
    }

    #[test]
    fn evaluate_utility_alpha_isolation_returns_alpha_times_deadline_term() {
        // β = γ = δ = 0 ⇒ u = α · deadline_term.
        let w = weights(0.42, 0.0, 0.0, 0.0);
        for &d_term in &[0.0_f64, 0.25, 0.5, 0.75, 1.0] {
            let got = evaluate_utility(&w, &terms(d_term, 0.99, 0.99, 0.99));
            let want = 0.42 * d_term;
            assert!((got - want).abs() < 1e-15);
        }
    }

    #[test]
    fn evaluate_utility_beta_isolation_returns_beta_times_fidelity_term() {
        // α = γ = δ = 0 ⇒ u = β · fidelity_term.
        let w = weights(0.0, 0.55, 0.0, 0.0);
        for &f_term in &[0.0_f64, 0.25, 0.5, 0.85, 1.0] {
            let got = evaluate_utility(&w, &terms(0.99, f_term, 0.99, 0.99));
            let want = 0.55 * f_term;
            assert!((got - want).abs() < 1e-15);
        }
    }

    #[test]
    fn evaluate_utility_gamma_isolation_returns_gamma_times_rivalry_term() {
        // α = β = δ = 0 ⇒ u = γ · rivalry_term.
        let w = weights(0.0, 0.0, 0.18, 0.0);
        for &r_term in &[0.0_f64, 0.3, 0.6, 0.95, 1.0] {
            let got = evaluate_utility(&w, &terms(0.99, 0.99, r_term, 0.99));
            let want = 0.18 * r_term;
            assert!((got - want).abs() < 1e-15);
        }
    }

    #[test]
    fn evaluate_utility_delta_isolation_returns_minus_delta_times_degradation() {
        // α = β = γ = 0 ⇒ u = − δ · degradation. The sign on
        // degradation is what makes it a *penalty*; test it
        // pointedly.
        let w = weights(0.0, 0.0, 0.0, 0.07);
        for &deg in &[0.0_f64, 0.1, 0.5, 1.0, 10.0] {
            let got = evaluate_utility(&w, &terms(0.99, 0.99, 0.99, deg));
            let want = -0.07 * deg;
            assert!(
                (got - want).abs() < 1e-15,
                "δ-isolation: expected {want}, got {got}",
            );
        }
    }

    #[test]
    fn evaluate_utility_with_all_zero_weights_returns_zero_for_any_terms() {
        let w = weights(0.0, 0.0, 0.0, 0.0);
        let t = terms(0.7, 0.8, 0.9, 100.0);
        assert_eq!(evaluate_utility(&w, &t), 0.0);
    }

    #[test]
    fn evaluate_utility_is_monotone_in_each_positive_term() {
        // Each positive-weight coefficient is monotone non-
        // decreasing in its term; this is the per-agent half of
        // the bargaining-round monotonicity proptest in T4.8.
        let w = weights(0.3, 0.4, 0.2, 0.1);
        let base = terms(0.5, 0.5, 0.5, 0.5);
        for axis in 0..3 {
            let mut bumped = base;
            match axis {
                0 => bumped.deadline_term += 0.1,
                1 => bumped.fidelity_term += 0.1,
                2 => bumped.rivalry_term += 0.1,
                _ => unreachable!(),
            };
            let before = evaluate_utility(&w, &base);
            let after = evaluate_utility(&w, &bumped);
            assert!(after >= before, "axis {axis}: monotone violation");
        }
    }

    #[test]
    fn evaluate_utility_is_anti_monotone_in_degradation() {
        let w = weights(0.3, 0.4, 0.2, 0.1);
        let base = terms(0.5, 0.5, 0.5, 0.0);
        let bumped = terms(0.5, 0.5, 0.5, 1.0);
        let before = evaluate_utility(&w, &base);
        let after = evaluate_utility(&w, &bumped);
        assert!(
            after < before,
            "δ axis must be strictly anti-monotone in degradation: {after} ≥ {before}",
        );
    }

    #[test]
    fn category_weights_pure_classical_zeroes_beta_and_delta() {
        // §6.3: category 5 (pure-classical) has β = 0 and δ = 0
        // — the workload carries no QPU side so the realised-
        // fidelity term and the QPU degradation penalty are
        // structurally zero.
        let w = CategoryWeights::default_for(Category::PureClassical);
        assert_eq!(w.beta, 0.0);
        assert_eq!(w.delta, 0.0);
        // α and γ are positive so the utility doesn't collapse
        // to identically zero.
        assert!(w.alpha > 0.0);
        assert!(w.gamma > 0.0);
    }

    #[test]
    fn category_weights_each_default_is_non_negative() {
        // The FLAG-C weights are all non-negative by
        // construction (δ enters with a minus sign in the
        // formula, not on the weight itself). Pin this so a
        // future tweak doesn't silently introduce a negative
        // weight that would flip the monotonicity contract.
        for c in [
            Category::Vqe,
            Category::Qaoa,
            Category::Qml,
            Category::QcMcQpe,
            Category::PureClassical,
            Category::Tomography,
        ] {
            let w = CategoryWeights::default_for(c);
            assert!(w.alpha >= 0.0, "{c:?}: α = {} is negative", w.alpha);
            assert!(w.beta >= 0.0, "{c:?}: β = {} is negative", w.beta);
            assert!(w.gamma >= 0.0, "{c:?}: γ = {} is negative", w.gamma);
            assert!(w.delta >= 0.0, "{c:?}: δ = {} is negative", w.delta);
        }
    }

    #[test]
    fn category_weights_deserialise_from_toml_block() {
        let toml = r#"
            alpha = 0.3
            beta = 0.5
            gamma = 0.15
            delta = 0.05
        "#;
        let w = CategoryWeights::from_toml(toml).expect("parse weights");
        assert_eq!(w, weights(0.3, 0.5, 0.15, 0.05));
    }

    #[test]
    fn utility_fn_trait_impl_matches_free_function() {
        // The CategoryWeights impl of UtilityFn must agree with
        // the free `evaluate_utility` function for every input.
        let w = weights(0.25, 0.4, 0.2, 0.15);
        let t = terms(0.6, 0.85, 0.7, 0.2);
        let via_fn = evaluate_utility(&w, &t);
        let via_trait = w.evaluate(&t);
        assert!((via_fn - via_trait).abs() < 1e-15);
    }

    #[test]
    fn utility_fn_disagreement_delegates_to_evaluate_on_the_fcfs_terms() {
        // FLAG-B closed: the disagreement-point uses the *same*
        // utility function, just on the FCFS-on-stale terms.
        // For any input `terms`, `disagreement(terms)` must
        // equal `evaluate(terms)` exactly.
        let w = weights(0.3, 0.5, 0.15, 0.05);
        for fixture in [
            terms(0.5, 0.5, 0.5, 0.5),
            terms(0.0, 0.0, 0.0, 0.0),
            terms(1.0, 1.0, 1.0, 0.0),
            terms(0.1, 0.9, 0.2, 5.0),
        ] {
            let via_disagreement = w.disagreement(&fixture);
            let via_evaluate = w.evaluate(&fixture);
            assert!(
                (via_disagreement - via_evaluate).abs() < 1e-15,
                "FLAG-B: disagreement({fixture:?}) {via_disagreement} != evaluate(...) {via_evaluate}",
            );
        }
    }

    /// T4.3 acceptance gate (the degenerate single-resource
    /// case from #49): on a scenario where the bargaining
    /// outcome and the FCFS-on-stale projection produce the
    /// **same** [`UtilityTerms`] — the only feasible allocation
    /// for a single resource and a single workflow — the
    /// bargained utility equals the disagreement utility, so
    /// the dominance inequality `bargained ≥ disagreement`
    /// holds with equality (no Pareto-improving move available;
    /// the bargain is the FCFS allocation).
    #[test]
    fn bargained_utility_dominates_disagreement_on_degenerate_single_resource_case() {
        let w = CategoryWeights::default_for(Category::Vqe);
        // Single-resource single-workflow: only one feasible
        // allocation. Both projections converge to the same
        // UtilityTerms.
        let only_feasible = terms(0.6, 0.85, 0.7, 0.2);
        let bargained = w.evaluate(&only_feasible);
        let disagreement = w.disagreement(&only_feasible);
        assert!(
            bargained >= disagreement,
            "FLAG-B disagreement-dominance violated on degenerate case: \
             bargained {bargained} < disagreement {disagreement}",
        );
        assert!(
            (bargained - disagreement).abs() < 1e-15,
            "degenerate case must satisfy equality, not strict inequality: \
             bargained {bargained} != disagreement {disagreement}",
        );
    }

    #[test]
    fn bargained_terms_pareto_dominating_fcfs_terms_yields_strict_advantage() {
        // Multi-resource case: when the bargaining outcome
        // delivers a Pareto-better term vector than the
        // FCFS-on-stale projection (every positive axis ≥, at
        // least one strictly >, and degradation ≤), the
        // bargained utility must strictly exceed the
        // disagreement utility.
        let w = weights(0.3, 0.5, 0.15, 0.05);
        let fcfs = terms(0.5, 0.7, 0.6, 0.3);
        let bargained = terms(
            /* deadline_term */ 0.8, // ↑ (more slack)
            /* fidelity_term */ 0.85, // ↑ (better channel)
            /* rivalry_term  */ 0.75, // ↑ (less rivalry)
            /* degradation   */ 0.2, // ↓ (less penalty)
        );
        let u_bargain = w.evaluate(&bargained);
        let u_disagree = w.disagreement(&fcfs);
        assert!(
            u_bargain > u_disagree,
            "bargained {u_bargain} ≯ disagreement {u_disagree} despite Pareto-dominance",
        );
    }

    #[test]
    fn disagreement_is_anti_monotone_in_degradation_term_just_like_evaluate() {
        // Because `disagreement` shares its formula with
        // `evaluate`, every monotonicity property carries over.
        // Pin the degradation axis as the canary so a future
        // refactor that diverges the two formulas trips.
        let w = weights(0.3, 0.5, 0.15, 0.05);
        let base = terms(0.5, 0.5, 0.5, 0.0);
        let bumped = terms(0.5, 0.5, 0.5, 1.0);
        let d_base = w.disagreement(&base);
        let d_bumped = w.disagreement(&bumped);
        assert!(
            d_bumped < d_base,
            "δ anti-monotonicity does not carry into disagreement: {d_bumped} ≥ {d_base}",
        );
    }
}
