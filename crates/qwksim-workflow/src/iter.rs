//! Iterative-loop runtime (cv2 convergence) per §5.3 of the
//! engineering plan.
//!
//! Each workflow of an iterative category (`Vqe`, `Qaoa`, `Qml`,
//! `QcMcQpe`, `Tomography`) runs a bounded outer loop. Per
//! iteration the runtime:
//!
//! 1. Samples a **Wiener-with-trend** noisy objective:
//!    `x_t = objective_0 + drift · t + σ · Z_t`,
//!    with `Z_t ~ Normal(0, 1)` drawn from the
//!    `Stream::ConvergenceObjective(workflow_id, iter_idx)`
//!    ChaCha20 stream (T0.12).
//! 2. Updates the EMA-smoothed objective
//!    `s_t = α · x_t + (1 − α) · s_{t−1}` with
//!    `α = 0.4` by default.
//! 3. Returns one of:
//!    - `Continue` — keep iterating;
//!    - `Converged { ... }` — the smoothed objective has dropped
//!      below the workflow's declared threshold *and* the
//!      current iteration index ≥ `min_iters`;
//!    - `MaxIterReached { ... }` — the iteration index has
//!      reached `max_iters` regardless of objective state.
//!
//! `min_iters` ≤ `max_iters` is an invariant; the constructor
//! [`IterativeRunSpec::new`] panics on violation. The simulator
//! must therefore catch `MaxIterReached` and treat it as a
//! converged-but-flat workflow.
//!
//! The runtime is a pure function of `(spec, state, rng)` and
//! never touches the wall clock or the DES kernel — it is
//! orchestrated *from* the simulator, never owning the
//! simulator's runtime.

use rand_chacha::ChaCha20Rng;
use rand_core::RngCore;

/// Inputs to one iterative-loop run.
#[derive(Debug, Clone, Copy)]
pub struct IterativeRunSpec {
    /// Workflow identifier. Used to keep the diagnostic output
    /// readable and to derive `Stream::ConvergenceObjective` in
    /// the calling simulator.
    pub workflow_id: u64,
    /// Minimum number of iterations the runtime must execute
    /// before convergence is allowed. Bounded above by
    /// `max_iters`.
    pub min_iters: u32,
    /// Maximum number of iterations the runtime will execute
    /// before forcing termination. Strictly positive.
    pub max_iters: u32,
    /// Per-workflow declared convergence threshold. The
    /// runtime terminates when the smoothed objective drops at
    /// or below this value (and `min_iters` is satisfied).
    pub convergence_threshold: f64,
    /// Initial (pre-noise) objective value.
    pub initial_objective: f64,
    /// Per-iteration drift `μ` added to the running objective.
    /// Negative values give a workflow that improves toward
    /// convergence; positive values are an anti-convergent
    /// regime (the runtime will then exit via `MaxIterReached`).
    pub drift_per_iter: f64,
    /// Per-iteration noise standard deviation `σ`.
    pub noise_std: f64,
    /// EMA smoothing parameter `α ∈ (0, 1]`. Higher = more
    /// reactive to the latest sample; lower = more smoothing.
    /// Defaults to `0.4`.
    pub smoothing: f64,
}

impl IterativeRunSpec {
    /// Build a spec; panics on `min_iters > max_iters`,
    /// non-positive `max_iters`, non-positive `noise_std`, or
    /// `smoothing ∉ (0, 1]`.
    pub fn new(
        workflow_id: u64,
        min_iters: u32,
        max_iters: u32,
        convergence_threshold: f64,
        initial_objective: f64,
        drift_per_iter: f64,
        noise_std: f64,
    ) -> Self {
        assert!(
            max_iters > 0,
            "IterativeRunSpec::new: max_iters must be > 0; got {max_iters}"
        );
        assert!(
            min_iters <= max_iters,
            "IterativeRunSpec::new: min_iters ({min_iters}) > max_iters ({max_iters})"
        );
        assert!(
            noise_std >= 0.0 && noise_std.is_finite(),
            "IterativeRunSpec::new: noise_std must be ≥ 0 and finite; got {noise_std}"
        );
        Self {
            workflow_id,
            min_iters,
            max_iters,
            convergence_threshold,
            initial_objective,
            drift_per_iter,
            noise_std,
            smoothing: 0.4,
        }
    }

    /// Override the default EMA smoothing parameter. `α` must
    /// be in `(0, 1]`.
    pub fn with_smoothing(mut self, alpha: f64) -> Self {
        assert!(
            alpha > 0.0 && alpha <= 1.0 && alpha.is_finite(),
            "IterativeRunSpec::with_smoothing: alpha ∈ (0, 1]; got {alpha}"
        );
        self.smoothing = alpha;
        self
    }
}

/// Mutable per-run state the runtime threads through every
/// iteration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IterState {
    /// Iterations completed so far (0 = no iterations yet).
    pub iter_idx: u32,
    /// Raw objective sample from the most recent iteration.
    /// `f64::NAN` before any iteration has run.
    pub last_objective: f64,
    /// EMA-smoothed objective; seeded to `initial_objective` on
    /// fresh state.
    pub smoothed_objective: f64,
}

impl IterState {
    /// Fresh state aligned with `spec.initial_objective`.
    pub fn new(spec: &IterativeRunSpec) -> Self {
        Self {
            iter_idx: 0,
            last_objective: f64::NAN,
            smoothed_objective: spec.initial_objective,
        }
    }
}

/// Outcome of a single iteration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IterOutcome {
    /// Run another iteration.
    Continue,
    /// Smoothed objective dropped at or below
    /// `convergence_threshold` after at least `min_iters` runs.
    Converged { iter_idx: u32, final_smoothed: f64 },
    /// `max_iters` reached without converging.
    MaxIterReached { iter_idx: u32, final_smoothed: f64 },
}

impl IterOutcome {
    /// `true` for `Converged` and `MaxIterReached`; `false` for
    /// `Continue`.
    pub fn is_terminal(&self) -> bool {
        !matches!(self, IterOutcome::Continue)
    }

    /// `Some(iter_idx)` when the outcome is terminal; `None`
    /// for `Continue`.
    pub fn terminal_iter(&self) -> Option<u32> {
        match self {
            IterOutcome::Converged { iter_idx, .. }
            | IterOutcome::MaxIterReached { iter_idx, .. } => Some(*iter_idx),
            IterOutcome::Continue => None,
        }
    }
}

/// Sample U ∼ Uniform(0, 1) from `rng`, never exactly 0.
fn uniform_open_01(rng: &mut ChaCha20Rng) -> f64 {
    let bits = rng.next_u64() >> 11;
    let u = bits as f64 / (1u64 << 53) as f64;
    if u == 0.0 {
        f64::MIN_POSITIVE
    } else {
        u
    }
}

/// Sample one standard-normal variate via Box-Muller. The runtime
/// only consumes the first of the pair; consuming both would
/// require state-carrying machinery that doesn't compose with
/// the per-iteration ChaCha20 stream re-seeding.
fn sample_standard_normal(rng: &mut ChaCha20Rng) -> f64 {
    let u1 = uniform_open_01(rng);
    let u2 = uniform_open_01(rng);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

/// Run exactly one iteration of the iterative loop. Mutates
/// `state.iter_idx`, `last_objective`, `smoothed_objective`.
pub fn run_iteration(
    spec: &IterativeRunSpec,
    state: &mut IterState,
    rng: &mut ChaCha20Rng,
) -> IterOutcome {
    state.iter_idx = state.iter_idx.saturating_add(1);
    let t = state.iter_idx as f64;
    let z = sample_standard_normal(rng);
    let x = spec.initial_objective + spec.drift_per_iter * t + spec.noise_std * z;
    state.last_objective = x;
    state.smoothed_objective =
        spec.smoothing * x + (1.0 - spec.smoothing) * state.smoothed_objective;

    let converged =
        state.iter_idx >= spec.min_iters && state.smoothed_objective <= spec.convergence_threshold;
    if converged {
        IterOutcome::Converged {
            iter_idx: state.iter_idx,
            final_smoothed: state.smoothed_objective,
        }
    } else if state.iter_idx >= spec.max_iters {
        IterOutcome::MaxIterReached {
            iter_idx: state.iter_idx,
            final_smoothed: state.smoothed_objective,
        }
    } else {
        IterOutcome::Continue
    }
}

/// Drive the iterative loop to termination. Returns the final
/// outcome (always either `Converged` or `MaxIterReached`).
pub fn run_until_termination(spec: &IterativeRunSpec, rng: &mut ChaCha20Rng) -> IterOutcome {
    let mut state = IterState::new(spec);
    loop {
        let outcome = run_iteration(spec, &mut state, rng);
        if outcome.is_terminal() {
            return outcome;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::SeedableRng;

    fn rng(seed: u8) -> ChaCha20Rng {
        ChaCha20Rng::from_seed([seed; 32])
    }

    #[test]
    fn iter_state_new_seeds_with_initial_objective() {
        let spec = IterativeRunSpec::new(0, 1, 10, -1.0, 5.0, -0.1, 0.1);
        let s = IterState::new(&spec);
        assert_eq!(s.iter_idx, 0);
        assert!(s.last_objective.is_nan());
        assert_eq!(s.smoothed_objective, 5.0);
    }

    #[test]
    fn monotone_decreasing_objective_converges_within_max_iters() {
        // Strong downward drift, tight noise — the smoothed
        // objective drops below threshold well before max_iters.
        // Headline T2.8 acceptance: a monotone-noisy objective
        // terminates within max_iters.
        let spec = IterativeRunSpec::new(
            /* workflow_id */ 1, /* min_iters */ 1, /* max_iters */ 100,
            /* threshold */ 0.0, /* initial_objective */ 10.0,
            /* drift_per_iter */ -1.0, /* noise_std */ 0.1,
        );
        let outcome = run_until_termination(&spec, &mut rng(7));
        match outcome {
            IterOutcome::Converged { iter_idx, .. } => {
                assert!(iter_idx <= 100, "iter_idx = {iter_idx} exceeds max");
            }
            other => panic!("expected Converged, got {other:?}"),
        }
    }

    #[test]
    fn min_iters_floor_blocks_early_convergence() {
        // Initial objective already below threshold so any
        // iteration would otherwise converge immediately. With
        // min_iters = 5 the runtime must execute at least 5
        // iterations before reporting Converged.
        let spec = IterativeRunSpec::new(
            /* workflow_id */ 2, /* min_iters */ 5, /* max_iters */ 10,
            /* threshold */ 1000.0, /* initial_objective */ 0.0,
            /* drift_per_iter */ 0.0, /* noise_std */ 0.0,
        );
        let outcome = run_until_termination(&spec, &mut rng(11));
        match outcome {
            IterOutcome::Converged { iter_idx, .. } => assert_eq!(iter_idx, 5),
            other => panic!("expected Converged at iter 5, got {other:?}"),
        }
    }

    #[test]
    fn anti_convergent_drift_exits_via_max_iters() {
        // Positive drift (objective increases over iterations) —
        // the runtime never converges and must exit via
        // MaxIterReached at exactly max_iters.
        let spec = IterativeRunSpec::new(
            /* workflow_id */ 3, /* min_iters */ 1, /* max_iters */ 12,
            /* threshold */ -1.0, /* initial_objective */ 1.0,
            /* drift_per_iter */ 1.0, /* noise_std */ 0.0,
        );
        let outcome = run_until_termination(&spec, &mut rng(13));
        match outcome {
            IterOutcome::MaxIterReached { iter_idx, .. } => assert_eq!(iter_idx, 12),
            other => panic!("expected MaxIterReached, got {other:?}"),
        }
    }

    #[test]
    fn run_iteration_increments_iter_idx() {
        let spec = IterativeRunSpec::new(4, 1, 10, -1.0, 5.0, -0.1, 0.1);
        let mut state = IterState::new(&spec);
        let mut r = rng(17);
        for expected in 1..=5 {
            run_iteration(&spec, &mut state, &mut r);
            assert_eq!(state.iter_idx, expected);
        }
    }

    #[test]
    fn outcome_helpers_distinguish_terminal_from_continue() {
        let cont = IterOutcome::Continue;
        let conv = IterOutcome::Converged {
            iter_idx: 7,
            final_smoothed: -0.1,
        };
        let cap = IterOutcome::MaxIterReached {
            iter_idx: 100,
            final_smoothed: 0.5,
        };
        assert!(!cont.is_terminal());
        assert!(conv.is_terminal());
        assert!(cap.is_terminal());
        assert_eq!(cont.terminal_iter(), None);
        assert_eq!(conv.terminal_iter(), Some(7));
        assert_eq!(cap.terminal_iter(), Some(100));
    }

    #[test]
    #[should_panic(expected = "max_iters must be > 0")]
    fn zero_max_iters_rejected() {
        IterativeRunSpec::new(0, 0, 0, -1.0, 1.0, -0.1, 0.1);
    }

    #[test]
    #[should_panic(expected = "min_iters")]
    fn min_iters_greater_than_max_rejected() {
        IterativeRunSpec::new(0, 10, 5, -1.0, 1.0, -0.1, 0.1);
    }

    #[test]
    #[should_panic(expected = "noise_std must be ≥ 0")]
    fn negative_noise_std_rejected() {
        IterativeRunSpec::new(0, 1, 10, -1.0, 1.0, -0.1, -0.1);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;
    use rand_core::SeedableRng;

    proptest! {
        /// **T2.8 headline acceptance gate.** For *any* seed and
        /// any spec drawn from the strategy below, the iterative
        /// runtime must terminate within `max_iters` iterations.
        /// Termination = either `Converged` or `MaxIterReached`;
        /// running forever or exceeding `max_iters` are both
        /// regressions.
        #[test]
        fn run_until_termination_always_terminates_within_max_iters(
            workflow_id in 0u64..1_000_000,
            min_iters in 1u32..50,
            extra_iters in 0u32..200,
            initial_objective in -10.0f64..10.0,
            drift in -2.0f64..2.0,
            noise_std in 0.0f64..3.0,
            seed_byte in any::<u8>(),
        ) {
            let max_iters = min_iters + extra_iters;
            let spec = IterativeRunSpec::new(
                workflow_id,
                min_iters,
                max_iters,
                /* threshold */ 0.0,
                initial_objective,
                drift,
                noise_std,
            );
            let mut rng = ChaCha20Rng::from_seed([seed_byte; 32]);
            let outcome = run_until_termination(&spec, &mut rng);
            let iter_idx = outcome
                .terminal_iter()
                .expect("run_until_termination must return a terminal outcome");
            prop_assert!(
                iter_idx >= spec.min_iters,
                "iter_idx ({iter_idx}) < min_iters ({})",
                spec.min_iters,
            );
            prop_assert!(
                iter_idx <= spec.max_iters,
                "iter_idx ({iter_idx}) > max_iters ({})",
                spec.max_iters,
            );
        }
    }
}
