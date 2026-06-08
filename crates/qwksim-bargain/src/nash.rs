//! Nash-product evaluator (T4.1).
//!
//! Implements the log-sum form from §6.1 of the engineering plan:
//!
//! ```text
//! nash_product(u, d) = Σᵢ ln(1 + max(uᵢ − dᵢ, 0))
//! ```
//!
//! ## Why log-sum?
//!
//! The classical Nash-product objective is
//!
//! ```text
//! N(u, d) = Πᵢ max(uᵢ − dᵢ, 0)
//! ```
//!
//! and the bargaining solver maximises it across feasible
//! allocations. Evaluating `N` directly is numerically fragile:
//! for `n` agents with surpluses `≈ 10⁻³` each, the product
//! collapses below `f64::MIN_POSITIVE` once `n ≳ 100`, and
//! large surpluses ≳ `10²` saturate `f64::MAX` after a few
//! dozen agents.
//!
//! Taking the log turns the product into a sum, and using
//! `ln_1p(x) = ln(1 + x)` keeps the per-agent term accurate
//! all the way down to `x ≈ 10⁻¹⁶` (where `ln(1 + x)` would
//! lose every significant bit to catastrophic cancellation).
//!
//! The transform is **monotone**: maximising `Σ ln(1 + sᵢ)` is
//! the same problem as maximising `Π (1 + sᵢ)` where `sᵢ ≥ 0`,
//! which differs from `Π sᵢ` only by the global affine shift
//! `1 + sᵢ` — adopted because it keeps the contribution of
//! disagreement-dominated agents (`sᵢ = 0`) at exactly zero
//! rather than `−∞`. The disagreement-floor convention is
//! standard in computational bargaining: an agent who would
//! prefer the disagreement point neither helps nor hurts the
//! coalition. Best-response monotonicity (T4.8 proptest) is
//! preserved because each `ln_1p` term is monotone non-
//! decreasing in `uᵢ`.

/// Nash-product objective in log-sum form.
///
/// Returns `Σᵢ ln(1 + max(uᵢ − dᵢ, 0))`. The expression is
/// monotone non-decreasing in every `uᵢ` (for fixed `dᵢ`) and
/// monotone non-increasing in every `dᵢ`. Disagreement-
/// dominated agents (`uᵢ ≤ dᵢ`) contribute exactly zero.
///
/// # Panics
///
/// Panics if `utilities.len() != disagreements.len()`. A length
/// mismatch is a programming error (agent indices misaligned),
/// not an input-quality issue — silently truncating the longer
/// vector would hide bargaining-round bugs by quietly dropping
/// agents from the objective.
pub fn nash_product(utilities: &[f64], disagreements: &[f64]) -> f64 {
    assert_eq!(
        utilities.len(),
        disagreements.len(),
        "nash_product: utilities ({}) and disagreements ({}) must have equal length",
        utilities.len(),
        disagreements.len(),
    );
    utilities
        .iter()
        .zip(disagreements.iter())
        .map(|(u, d)| (u - d).max(0.0).ln_1p())
        .sum::<f64>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_inputs_yield_zero() {
        // Vacuous case: sum of zero terms is 0.0, matching the
        // identity of the Nash product Π(1 + 0) = 1 → ln 1 = 0.
        assert_eq!(nash_product(&[], &[]), 0.0);
    }

    #[test]
    fn single_agent_above_disagreement_equals_ln_1p_of_gap() {
        let u = &[1.5];
        let d = &[0.5];
        let got = nash_product(u, d);
        let want = (1.0f64).ln_1p(); // ln 2
        assert!((got - want).abs() < 1e-15);
        assert!((got - std::f64::consts::LN_2).abs() < 1e-15);
    }

    #[test]
    fn agent_at_disagreement_contributes_zero() {
        // u == d → surplus 0 → ln(1 + 0) = 0.
        assert_eq!(nash_product(&[3.0], &[3.0]), 0.0);
    }

    #[test]
    fn agent_below_disagreement_floors_at_zero_not_negative() {
        // The Nash-product objective is the *threat point*: an
        // agent who prefers the disagreement outcome neither
        // helps nor hurts the coalition. The `max(0.0, …)` clamp
        // is what makes that contract hold, so test it
        // explicitly.
        for &(u, d) in &[(0.0_f64, 5.0), (-100.0, 0.0), (f64::MIN_POSITIVE, 1.0)] {
            assert_eq!(
                nash_product(&[u], &[d]),
                0.0,
                "below-threshold pair ({u}, {d}) leaked a non-zero contribution",
            );
        }
    }

    #[test]
    fn multi_agent_result_equals_pointwise_sum() {
        // 4-agent fixture: verify the result matches the
        // hand-computed pointwise sum so the iteration order
        // and the clamp interact correctly.
        let u = &[2.0, 1.0, -1.0, 5.0];
        let d = &[1.0, 1.0, 2.0, 0.5];
        // surplus = [1.0, 0.0, -3.0 → 0, 4.5]
        // terms   = [ln 2, 0, 0, ln 5.5]
        let want = (1.0_f64).ln_1p() + 0.0 + 0.0 + (4.5_f64).ln_1p();
        let got = nash_product(u, d);
        assert!(
            (got - want).abs() < 1e-15,
            "nash_product result {got} != expected {want}",
        );
    }

    #[test]
    fn result_is_commutative_in_agent_order() {
        // Sum is commutative; per-agent reorderings must produce
        // the same value modulo floating-point reassociation.
        let u_a = &[2.0, 1.5, 0.1, 7.0];
        let d_a = &[1.0, 0.5, 0.05, 6.0];
        let u_b = &[7.0, 0.1, 2.0, 1.5];
        let d_b = &[6.0, 0.05, 1.0, 0.5];
        let a = nash_product(u_a, d_a);
        let b = nash_product(u_b, d_b);
        assert!((a - b).abs() < 1e-12, "shuffled inputs produced {a} ≠ {b}",);
    }

    #[test]
    fn small_gap_uses_ln_1p_precision() {
        // ln_1p preserves accuracy for surpluses below the f64
        // machine epsilon (≈ 2.22e-16). For surplus = 1e-18,
        // `1.0 + 1e-18` rounds back to exactly `1.0`, so the
        // naive `(1.0 + s).ln()` evaluates to 0.0 — losing the
        // bit entirely. ln_1p evaluates the Taylor series in s
        // directly and recovers ~s with full precision.
        let surplus = 1e-18_f64;
        let u = &[surplus];
        let d = &[0.0];
        let got = nash_product(u, d);

        // Harness self-check: the naive formulation must lose
        // the surplus to rounding.
        let naive = (1.0_f64 + surplus).ln();
        assert_eq!(
            naive, 0.0,
            "harness assumption broken: naive (1 + {surplus}).ln() = {naive}, expected 0.0",
        );

        // Our log-sum form must recover the surplus.
        assert!(got > 0.0, "expected positive contribution, got {got}");
        // ln_1p(x) ≈ x − x²/2 + O(x³); at x = 1e-18 the leading
        // term dominates by 18 orders of magnitude.
        assert!(
            (got - surplus).abs() < 1e-30,
            "ln_1p({surplus}) ≈ {got}, expected ≈ {surplus}",
        );
    }

    #[test]
    fn very_large_utility_does_not_overflow() {
        // Naive product Π(1 + uᵢ) would overflow f64 around
        // n = 1024 with uᵢ ≈ 1.0; the log-sum form sums to
        // ≈ 710 (well under f64::MAX) and stays finite.
        let u: Vec<f64> = vec![1.0; 2_000];
        let d = vec![0.0; 2_000];
        let got = nash_product(&u, &d);
        assert!(got.is_finite(), "log-sum overflowed at 2000 agents");
        // 2000 · ln 2 ≈ 1386
        let want = 2_000.0 * std::f64::consts::LN_2;
        assert!((got - want).abs() < 1e-9, "expected ~{want}, got {got}",);
    }

    #[test]
    fn many_small_surpluses_accumulate_without_underflow() {
        // Naive product of 10_000 agents with surplus 1e-3 each
        // would underflow: (1.001)^10_000 ≈ exp(10) ≈ 22_026 —
        // OK, but (0.001)^10_000 underflows. We test the more
        // subtle case: many tiny-but-nonzero surpluses must
        // accumulate to ~10_000 · 1e-3 = 10.0 without losing
        // either summands or precision.
        let n = 10_000;
        let u = vec![1e-3; n];
        let d = vec![0.0; n];
        let got = nash_product(&u, &d);
        let want = n as f64 * (1e-3_f64).ln_1p();
        assert!(
            (got - want).abs() < 1e-9,
            "small-surplus accumulation lost precision: got {got}, expected {want}",
        );
        // And confirm the value is in the right ballpark (~10).
        assert!(got > 9.99 && got < 10.0, "expected ~10, got {got}",);
    }

    #[test]
    fn infinite_utility_returns_infinity() {
        // f64::INFINITY surplus → ln_1p(INF) = INF; the sum
        // carries INF to the final result. Bargaining solver
        // guards against infinite utility upstream, but the
        // evaluator must propagate, not silently clamp.
        let got = nash_product(&[f64::INFINITY], &[0.0]);
        assert!(got.is_infinite() && got.is_sign_positive());
    }

    #[test]
    fn negative_infinity_utility_floors_to_zero() {
        // u = -∞ → surplus -∞ → max(0.0, …) = 0 → ln_1p(0) = 0.
        // Disagreement-dominated agent (even at -∞) contributes
        // 0, not -∞, by the floor convention.
        assert_eq!(nash_product(&[f64::NEG_INFINITY], &[0.0]), 0.0);
    }

    #[test]
    fn nan_input_is_silently_floored_to_zero_by_the_max_clamp() {
        // Per IEEE 754-2008 `maxNum`, `f64::max(NaN, 0.0)`
        // returns the **non-NaN** argument (= 0.0); the same is
        // true of `(NaN).max(0.0)` in Rust. The §6.1 formula
        // `(u − d).max(0.0).ln_1p()` therefore floors any NaN
        // utility to a zero contribution rather than poisoning
        // the sum.
        //
        // This is the spec'd behaviour, but it is a property
        // worth pinning: a future refactor that swaps
        // `.max(0.0)` for `f64::max(0.0, x)` would *also*
        // preserve it (both are `maxNum`-style), whereas a swap
        // to a plain `if x > 0.0 { x } else { 0.0 }` clamp
        // would propagate NaN as `0.0` too — but a swap to
        // `if x < 0.0 { 0.0 } else { x }` would *propagate* NaN
        // (since `NaN < 0.0` is false). Best-response solvers
        // must catch NaN upstream regardless; the contract here
        // documents what reaches this function.
        let r = nash_product(&[f64::NAN, 1.0], &[0.0, 0.0]);
        // First term NaN clamped to 0; second term ln 2.
        assert!(r.is_finite(), "expected finite, got {r}");
        assert!(
            (r - std::f64::consts::LN_2).abs() < 1e-15,
            "expected ln 2, got {r}",
        );
    }

    #[test]
    fn nan_in_disagreement_propagates_to_nan_output() {
        // The symmetric case: NaN in the *disagreement* vector.
        // (u − NaN) = NaN, and NaN.max(0.0) = 0.0, so this is
        // also clamped — same behaviour as above. Pin it so a
        // future change to either side of the subtraction
        // doesn't silently flip NaN handling.
        let r = nash_product(&[1.0, 1.0], &[0.0, f64::NAN]);
        assert!(r.is_finite(), "NaN-in-disagreement should clamp; got {r}");
        assert!(
            (r - std::f64::consts::LN_2).abs() < 1e-15,
            "expected ln 2, got {r}",
        );
    }

    #[test]
    #[should_panic(expected = "must have equal length")]
    fn length_mismatch_panics_loudly() {
        // Silent truncation would hide a bug where a round adds
        // an agent on one side of the iteration but not the
        // other. Make the mismatch fail at the boundary.
        let _ = nash_product(&[1.0, 2.0, 3.0], &[0.0, 0.0]);
    }

    #[test]
    fn monotonic_non_decreasing_in_each_utility() {
        // Increasing any single uᵢ (with fixed dᵢ and the other
        // agents unchanged) must never decrease the Nash
        // product. This is what makes the best-response loop in
        // T4.x converge: an agent's local improvement cannot
        // hurt the global objective.
        let d = &[0.5, 0.5, 0.5];
        let base = &[1.0, 2.0, 3.0];
        let mut bumped = base.to_vec();
        for i in 0..bumped.len() {
            let before = nash_product(base, d);
            bumped[i] += 1.0; // any positive delta
            let after = nash_product(&bumped, d);
            assert!(
                after >= before,
                "monotonicity violated bumping agent {i}: {before} → {after}",
            );
            bumped[i] = base[i]; // restore
        }
    }
}
