//! OU calibration drift integrator (T3.3).
//!
//! Each fidelity channel of a QPU carries an Ornstein–Uhlenbeck
//! drift state. Calibration anchors set the long-term mean μ;
//! between calibration boundaries the realised fidelity diffuses
//! around μ with mean-reversion strength θ and per-step noise
//! σ. Step size is fixed at `Δt = 1 sec` per the §13.3 spec.
//!
//! ## Discretisation
//!
//! Unit-step Euler–Maruyama:
//!
//! ```text
//! X_{n+1} = X_n + θ·(μ − X_n) + σ·Z_n,    Z_n ~ N(0,1) iid
//! ```
//!
//! This is exactly the AR(1) recurrence `X_{n+1} = (1−θ)·X_n +
//! θ·μ + σ·Z_n`, so the stationary statistics are
//!
//! ```text
//! E[X_∞]   = μ
//! Var[X_∞] = σ² / (1 − (1 − θ)²) = σ² / (θ·(2 − θ))
//! ```
//!
//! and the lag-1 autocorrelation is `1 − θ`. The T3.3 acceptance
//! test asserts the empirical mean and variance of a 10⁵-step
//! trace land within 2% of those analytical values.
//!
//! ## Determinism (Q6′ = R2)
//!
//! Each channel owns its own [`ChaCha20Rng`] seeded from
//! [`StreamId::CalibrationDrift`] via the workspace
//! [`RngHierarchy`] — so two integrators with the same
//! `(master_seed, replicate_index, site_id, qpu_id, channel)`
//! produce byte-identical traces. The standard-normal sampler is
//! a polar-free Box–Muller mirroring the workspace pattern (see
//! `qwksim-resources::advertisement::sample_standard_normal`).
//!
//! ## Step-on-demand
//!
//! Callers query [`OuDriftState::current_state`] with the
//! simulator's wall-clock time in nanoseconds. The integrator
//! advances the channel forward by `floor((t_ns − last_t_ns) /
//! step_dt_ns)` whole steps before returning `x`; if the caller
//! polls at sub-step granularity the channel state is held
//! constant within the step. This matches the "Δt = 1 sec"
//! discretisation declared in the spec.

use std::collections::BTreeMap;

use rand_chacha::ChaCha20Rng;
use rand_core::RngCore;

use qwksim_core::event::SimTime;
use qwksim_core::rng::{ReplicateRng, StreamId};

/// One nanosecond per second, used as the default OU step
/// duration `Δt`.
pub const OU_STEP_DT_NS: u64 = 1_000_000_000;

/// QPU fidelity channels the calibration drift tracks. Maps to
/// the third coordinate of [`StreamId::CalibrationDrift`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FidelityChannel {
    /// Single-qubit gate fidelity channel.
    OneQubit,
    /// Two-qubit gate fidelity channel.
    TwoQubit,
    /// Mid-circuit readout fidelity channel.
    Readout,
}

impl FidelityChannel {
    /// Stable byte tag used as the `channel` field of
    /// [`StreamId::CalibrationDrift`]. Never reorder these — they
    /// are part of the deterministic-replay contract.
    pub fn stream_tag(self) -> u8 {
        match self {
            FidelityChannel::OneQubit => 0,
            FidelityChannel::TwoQubit => 1,
            FidelityChannel::Readout => 2,
        }
    }
}

/// Per-channel OU parameters (unit-step Euler–Maruyama).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OuParams {
    /// Mean-reversion strength `θ ∈ (0, 1]`.
    pub theta: f64,
    /// Long-term mean `μ` (typically the anchor's nominal
    /// fidelity for that channel).
    pub mu: f64,
    /// Per-step noise scale `σ ≥ 0`.
    pub sigma: f64,
}

impl OuParams {
    /// Build a checked parameter set.
    ///
    /// # Panics
    /// Panics if `theta ∉ (0, 1]`, `sigma < 0`, or any value is
    /// non-finite. The integrator is unstable outside that range
    /// (no mean-reversion at `θ = 0`; oscillatory divergence at
    /// `θ > 1`).
    pub fn new(theta: f64, mu: f64, sigma: f64) -> Self {
        assert!(
            theta.is_finite() && theta > 0.0 && theta <= 1.0,
            "OU theta must be in (0, 1]; got {theta}",
        );
        assert!(mu.is_finite(), "OU mu must be finite; got {mu}");
        assert!(
            sigma.is_finite() && sigma >= 0.0,
            "OU sigma must be finite and ≥ 0; got {sigma}",
        );
        Self { theta, mu, sigma }
    }

    /// Analytical stationary variance `σ² / (θ·(2 − θ))`. Used by
    /// the acceptance-criterion test.
    pub fn stationary_variance(self) -> f64 {
        let denom = self.theta * (2.0 - self.theta);
        self.sigma * self.sigma / denom
    }
}

/// State of one fidelity channel's OU integrator.
#[derive(Debug, Clone)]
pub struct OuChannelState {
    params: OuParams,
    x: f64,
    /// Last wall-clock time we integrated up to (ns). Steps
    /// already applied = `(last_t_ns − t0_ns) / step_dt_ns`.
    last_t_ns: SimTime,
    rng: ChaCha20Rng,
}

impl OuChannelState {
    /// Build a channel state at the long-term mean with the
    /// caller-supplied rng. The integrator is exposed for
    /// hand-rolled tests; production callers should use
    /// [`OuDriftState::new`].
    pub fn new(params: OuParams, x0: f64, t0_ns: SimTime, rng: ChaCha20Rng) -> Self {
        Self {
            params,
            x: x0,
            last_t_ns: t0_ns,
            rng,
        }
    }

    /// Current parameters.
    pub fn params(&self) -> OuParams {
        self.params
    }

    /// Last wall-clock time the channel was integrated up to.
    pub fn last_t_ns(&self) -> SimTime {
        self.last_t_ns
    }

    /// Realised fidelity at the most-recently integrated step
    /// (no further integration).
    pub fn current_value(&self) -> f64 {
        self.x
    }

    /// Advance the channel by exactly one Euler–Maruyama step.
    ///
    /// The increment is `θ·(μ − X) + σ·Z` with `Z ~ N(0,1)`
    /// drawn from the channel's own rng. Bumps `last_t_ns` by
    /// `step_dt_ns`.
    pub fn step(&mut self, step_dt_ns: u64) {
        let z = sample_standard_normal(&mut self.rng);
        let dx = self.params.theta * (self.params.mu - self.x) + self.params.sigma * z;
        self.x += dx;
        self.last_t_ns = self.last_t_ns.saturating_add(step_dt_ns);
    }

    /// Reset this channel's realised value back to the long-term
    /// mean `μ`, anchoring `last_t_ns` at `t_ns`. Models the
    /// abrupt "fresh calibration" event at each calibration
    /// boundary (T3.4).
    ///
    /// The channel's rng is **not** rewound: subsequent steps
    /// continue to consume the stream deterministically so a
    /// post-reset trace replayed under CRN is bit-identical to
    /// the original.
    pub fn reset_to_mu_at(&mut self, t_ns: SimTime) {
        self.x = self.params.mu;
        self.last_t_ns = t_ns;
    }
}

/// All fidelity channels of one QPU, sharing the same step size
/// and indexed by [`FidelityChannel`]. Step-on-demand: callers
/// query [`OuDriftState::current_state`] with a simulator-time
/// stamp and the integrator brings the requested channel up to
/// that step floor before returning.
#[derive(Debug, Clone)]
pub struct OuDriftState {
    channels: BTreeMap<FidelityChannel, OuChannelState>,
    step_dt_ns: u64,
    /// Simulator time of the most recent calibration boundary
    /// reset that this drift state has acknowledged. `0` at
    /// construction (the QPU comes online freshly calibrated).
    last_reset_at_ns: SimTime,
}

impl OuDriftState {
    /// Build a drift state with one OU channel per
    /// `(channel, params, x0)` entry. Each channel's rng is
    /// derived from the workspace [`StreamId::CalibrationDrift`]
    /// keyed by `(site_id, qpu_id, channel.stream_tag())` so the
    /// trace is reproducible from the master seed.
    pub fn new(
        replicate: ReplicateRng,
        site_id: u16,
        qpu_id: u16,
        t0_ns: SimTime,
        step_dt_ns: u64,
        channels: &[(FidelityChannel, OuParams, f64)],
    ) -> Self {
        assert!(step_dt_ns > 0, "OU step_dt_ns must be > 0");
        let mut map: BTreeMap<FidelityChannel, OuChannelState> = BTreeMap::new();
        for &(channel, params, x0) in channels {
            let rng = replicate.stream(StreamId::CalibrationDrift {
                site_id,
                qpu_id,
                channel: channel.stream_tag(),
            });
            let prev = map.insert(channel, OuChannelState::new(params, x0, t0_ns, rng));
            debug_assert!(
                prev.is_none(),
                "OuDriftState: duplicate channel {channel:?} in initialiser",
            );
        }
        Self {
            channels: map,
            step_dt_ns,
            last_reset_at_ns: t0_ns,
        }
    }

    /// Step size in nanoseconds.
    pub fn step_dt_ns(&self) -> u64 {
        self.step_dt_ns
    }

    /// Read-only access to channel state (useful for tests).
    pub fn channel(&self, channel: FidelityChannel) -> Option<&OuChannelState> {
        self.channels.get(&channel)
    }

    /// Simulator time of the most recent calibration-boundary
    /// reset applied to this drift state.
    pub fn last_reset_at_ns(&self) -> SimTime {
        self.last_reset_at_ns
    }

    /// Reset every channel to its long-term mean `μ`, stamping
    /// `t_ns` as the new boundary. Used by the QpuAgent to apply
    /// a calibration boundary reset (T3.4).
    pub fn reset_to_nominal_at(&mut self, t_ns: SimTime) {
        for st in self.channels.values_mut() {
            st.reset_to_mu_at(t_ns);
        }
        self.last_reset_at_ns = t_ns;
    }

    /// Step-on-demand. Returns the realised fidelity for
    /// `channel` at simulator time `t_ns`, advancing the
    /// channel forward by as many whole `step_dt_ns` steps as
    /// the gap to the previous query allows.
    ///
    /// # Panics
    /// Panics if `channel` was not registered at construction
    /// time, or if `t_ns < last_t_ns` (time must be monotonic
    /// non-decreasing within a single replicate).
    pub fn current_state(&mut self, channel: FidelityChannel, t_ns: SimTime) -> f64 {
        let dt_ns = self.step_dt_ns;
        let st = self
            .channels
            .get_mut(&channel)
            .unwrap_or_else(|| panic!("OuDriftState: channel {channel:?} not registered"));
        assert!(
            t_ns >= st.last_t_ns,
            "OuDriftState: non-monotonic query {t_ns} < last {} on {channel:?}",
            st.last_t_ns,
        );
        let gap_ns = t_ns - st.last_t_ns;
        let steps = gap_ns / dt_ns;
        for _ in 0..steps {
            st.step(dt_ns);
        }
        st.x
    }
}

/// Sample `U ∈ (0, 1]` from `rng`. Mirrors the workspace pattern
/// in `qwksim-resources::advertisement::uniform_open_01`.
fn uniform_open_01(rng: &mut ChaCha20Rng) -> f64 {
    let bits = rng.next_u64() >> 11;
    let u = bits as f64 / (1u64 << 53) as f64;
    if u == 0.0 {
        f64::MIN_POSITIVE
    } else {
        u
    }
}

/// Standard-normal via Box–Muller. Each call consumes two
/// `u64`s from `rng` and returns one normal variate (the
/// companion variate is discarded for simplicity — the channel
/// rngs are decorrelated per-channel via the stream-id, so this
/// is not a correlation source).
fn sample_standard_normal(rng: &mut ChaCha20Rng) -> f64 {
    let u1 = uniform_open_01(rng);
    let u2 = uniform_open_01(rng);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwksim_core::rng::RngHierarchy;

    fn rng_for(seed: u64, channel: FidelityChannel) -> ChaCha20Rng {
        RngHierarchy::new(seed)
            .replicate(0)
            .stream(StreamId::CalibrationDrift {
                site_id: 0,
                qpu_id: 0,
                channel: channel.stream_tag(),
            })
    }

    #[test]
    fn ou_params_constructor_validates_inputs() {
        let _ok = OuParams::new(0.2, 0.99, 0.01);
        assert!(std::panic::catch_unwind(|| OuParams::new(0.0, 0.99, 0.01)).is_err());
        assert!(std::panic::catch_unwind(|| OuParams::new(1.5, 0.99, 0.01)).is_err());
        assert!(std::panic::catch_unwind(|| OuParams::new(0.2, 0.99, -0.01)).is_err());
        assert!(std::panic::catch_unwind(|| OuParams::new(f64::NAN, 0.99, 0.01)).is_err());
    }

    #[test]
    fn analytical_stationary_variance_matches_closed_form() {
        let p = OuParams::new(0.3, 0.99, 0.01);
        let expected = 1e-4 / (0.3 * (2.0 - 0.3));
        assert!((p.stationary_variance() - expected).abs() < 1e-15);
    }

    #[test]
    fn fidelity_channel_stream_tags_are_pinned() {
        // Stream-tag values are part of the deterministic-replay
        // contract; pin them so a future refactor cannot silently
        // re-key existing replay logs.
        assert_eq!(FidelityChannel::OneQubit.stream_tag(), 0);
        assert_eq!(FidelityChannel::TwoQubit.stream_tag(), 1);
        assert_eq!(FidelityChannel::Readout.stream_tag(), 2);
    }

    /// T3.3 acceptance gate: empirical mean and variance over
    /// 10⁵ steps match the analytical OU stationary values
    /// within 2%.
    #[test]
    fn trace_mean_and_variance_match_analytical_within_2_percent() {
        let params = OuParams::new(0.3, 0.99, 0.01);
        let mut st = OuChannelState::new(
            params,
            params.mu,
            0,
            rng_for(0xdead_beef, FidelityChannel::OneQubit),
        );

        // 1000-step burn-in (already at μ but cheap insurance).
        for _ in 0..1_000 {
            st.step(OU_STEP_DT_NS);
        }

        // Welford streaming mean/variance over 100_000 steps.
        let n: u64 = 100_000;
        let (mut mean, mut m2) = (0.0_f64, 0.0_f64);
        for k in 1..=n {
            st.step(OU_STEP_DT_NS);
            let x = st.current_value();
            let delta = x - mean;
            mean += delta / (k as f64);
            let delta2 = x - mean;
            m2 += delta * delta2;
        }
        let variance = m2 / (n as f64 - 1.0);

        let analytical_mean = params.mu;
        let analytical_var = params.stationary_variance();
        let mean_err = (mean - analytical_mean).abs() / analytical_mean;
        let var_err = (variance - analytical_var).abs() / analytical_var;

        assert!(
            mean_err < 0.02,
            "empirical mean {mean} deviates {:.4}% from analytical {analytical_mean} (>2%)",
            mean_err * 100.0,
        );
        assert!(
            var_err < 0.02,
            "empirical variance {variance} deviates {:.4}% from analytical {analytical_var} (>2%)",
            var_err * 100.0,
        );
    }

    /// Determinism under CRN: same seed → byte-identical trace.
    #[test]
    fn two_traces_with_same_seed_are_bit_identical() {
        let params = OuParams::new(0.3, 0.99, 0.01);
        let mut a = OuChannelState::new(
            params,
            params.mu,
            0,
            rng_for(0xfeed_face, FidelityChannel::OneQubit),
        );
        let mut b = OuChannelState::new(
            params,
            params.mu,
            0,
            rng_for(0xfeed_face, FidelityChannel::OneQubit),
        );

        for _ in 0..10_000 {
            a.step(OU_STEP_DT_NS);
            b.step(OU_STEP_DT_NS);
            assert_eq!(
                a.current_value().to_bits(),
                b.current_value().to_bits(),
                "CRN traces must be bit-identical",
            );
        }
    }

    /// Different seeds decorrelate (sanity check for the CRN
    /// test above).
    #[test]
    fn two_traces_with_different_seeds_diverge() {
        let params = OuParams::new(0.3, 0.99, 0.01);
        let mut a = OuChannelState::new(
            params,
            params.mu,
            0,
            rng_for(0xfeed_face, FidelityChannel::OneQubit),
        );
        let mut b = OuChannelState::new(
            params,
            params.mu,
            0,
            rng_for(0xfeed_face, FidelityChannel::TwoQubit),
        );
        let mut diverged = false;
        for _ in 0..100 {
            a.step(OU_STEP_DT_NS);
            b.step(OU_STEP_DT_NS);
            if a.current_value() != b.current_value() {
                diverged = true;
                break;
            }
        }
        assert!(
            diverged,
            "different channels must decorrelate within 100 steps"
        );
    }

    #[test]
    fn current_state_step_on_demand_floors_to_step_boundary() {
        let params = OuParams::new(0.3, 0.99, 0.01);
        let replicate = RngHierarchy::new(0xa5a5_a5a5).replicate(0);
        let mut drift = OuDriftState::new(
            replicate,
            0,
            0,
            /* t0_ns */ 0,
            OU_STEP_DT_NS,
            &[(FidelityChannel::OneQubit, params, params.mu)],
        );

        // Sub-step query → no integration (channel still at x0).
        let x0 = drift.current_state(FidelityChannel::OneQubit, OU_STEP_DT_NS - 1);
        assert_eq!(x0.to_bits(), params.mu.to_bits());

        // Exactly one step boundary → one EM step.
        let x1 = drift.current_state(FidelityChannel::OneQubit, OU_STEP_DT_NS);
        assert_ne!(x1.to_bits(), params.mu.to_bits(), "one EM step taken");

        // last_t_ns must be at the step boundary, not at the
        // sub-step query.
        let ch = drift
            .channel(FidelityChannel::OneQubit)
            .expect("registered");
        assert_eq!(ch.last_t_ns(), OU_STEP_DT_NS);
    }

    #[test]
    fn current_state_jumps_multiple_steps_in_one_query() {
        let params = OuParams::new(0.3, 0.99, 0.01);
        let replicate_a = RngHierarchy::new(0x1).replicate(0);
        let replicate_b = RngHierarchy::new(0x1).replicate(0);

        let mut drift_a = OuDriftState::new(
            replicate_a,
            0,
            0,
            0,
            OU_STEP_DT_NS,
            &[(FidelityChannel::OneQubit, params, params.mu)],
        );
        let mut drift_b = OuDriftState::new(
            replicate_b,
            0,
            0,
            0,
            OU_STEP_DT_NS,
            &[(FidelityChannel::OneQubit, params, params.mu)],
        );

        // A: 5 single-step queries.
        let mut x_a = 0.0_f64;
        for k in 1..=5 {
            x_a = drift_a.current_state(FidelityChannel::OneQubit, k * OU_STEP_DT_NS);
        }
        // B: one big-jump query that swallows all 5 steps.
        let x_b = drift_b.current_state(FidelityChannel::OneQubit, 5 * OU_STEP_DT_NS);

        assert_eq!(
            x_a.to_bits(),
            x_b.to_bits(),
            "step-on-demand must be invariant to query granularity",
        );
    }

    #[test]
    #[should_panic(expected = "non-monotonic")]
    fn non_monotonic_query_panics() {
        let params = OuParams::new(0.3, 0.99, 0.01);
        let replicate = RngHierarchy::new(0x2).replicate(0);
        let mut drift = OuDriftState::new(
            replicate,
            0,
            0,
            0,
            OU_STEP_DT_NS,
            &[(FidelityChannel::OneQubit, params, params.mu)],
        );
        drift.current_state(FidelityChannel::OneQubit, 10 * OU_STEP_DT_NS);
        drift.current_state(FidelityChannel::OneQubit, OU_STEP_DT_NS); // backwards
    }

    #[test]
    #[should_panic(expected = "not registered")]
    fn unregistered_channel_panics() {
        let params = OuParams::new(0.3, 0.99, 0.01);
        let replicate = RngHierarchy::new(0x3).replicate(0);
        let mut drift = OuDriftState::new(
            replicate,
            0,
            0,
            0,
            OU_STEP_DT_NS,
            &[(FidelityChannel::OneQubit, params, params.mu)],
        );
        drift.current_state(FidelityChannel::TwoQubit, OU_STEP_DT_NS);
    }
}
