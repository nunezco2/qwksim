//! Simulator-clock-driven advertisement broadcast — every per-
//! resource agent emits its [`AdvertisedSummary`] at a τ_adv
//! cadence, and downstream consumers see the summary after a
//! lognormal-latency delay drawn from
//! `Stream::LinkLatency(src_site, dst_site)` (T0.12 RNG
//! hierarchy).
//!
//! T2.10 ships a **pure-function scheduler** that returns the
//! list of `(broadcast_at, delivered_at, summary)` tuples that
//! the DES kernel will dispatch. Integration with the
//! `Simulator`'s event queue lands in T4.x alongside the full
//! advertisement protocol — today the bargaining solver does
//! not yet exist, and the test fixture in `tests/` exercises
//! the scheduler directly.
//!
//! The lognormal latency uses the **mean-preserving**
//! parameterisation: `μ = ln(mean) − σ²/2` so the arithmetic
//! mean of the per-broadcast latency distribution equals
//! `params.mean_ns` exactly.

use rand_chacha::ChaCha20Rng;
use rand_core::RngCore;

use qwksim_core::event::SimTime;
use qwksim_core::rng::{RngHierarchy, StreamId};

use crate::AdvertisedSummary;

/// Per-link lognormal-latency parameters. `mean_ns` is the
/// **arithmetic mean** (not the median); `log_sigma` is the
/// standard deviation in log-space.
#[derive(Debug, Clone, Copy)]
pub struct LinkLatencyParams {
    /// Arithmetic mean of the lognormal distribution
    /// (simulator nanoseconds).
    pub mean_ns: f64,
    /// Standard deviation in log-space; default 0.3 (≈ 35 %
    /// arithmetic CV).
    pub log_sigma: f64,
}

impl LinkLatencyParams {
    pub fn new(mean_ns: f64, log_sigma: f64) -> Self {
        assert!(
            mean_ns > 0.0 && mean_ns.is_finite(),
            "LinkLatencyParams::new: mean_ns must be > 0 and finite; got {mean_ns}"
        );
        assert!(
            log_sigma >= 0.0 && log_sigma.is_finite(),
            "LinkLatencyParams::new: log_sigma must be ≥ 0 and finite; got {log_sigma}"
        );
        Self { mean_ns, log_sigma }
    }
}

/// One `(broadcast, delivered)` pair plus the summary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdvertisementBroadcast {
    /// Simulator time at which the source agent emitted the
    /// summary.
    pub broadcast_at_ns: SimTime,
    /// Simulator time at which the consumer observes the
    /// summary (`broadcast_at_ns + lognormal-latency`).
    pub delivered_at_ns: SimTime,
    /// The summary the consumer observes.
    pub summary: AdvertisedSummary,
}

/// Identifier shape for a τ_adv broadcast schedule.
#[derive(Debug, Clone, Copy)]
pub struct AdvertisementChannel {
    pub src_site: u16,
    pub dst_site: u16,
    pub tau_adv_ns: SimTime,
    pub link: LinkLatencyParams,
}

impl AdvertisementChannel {
    /// Build a channel; panics if `tau_adv_ns == 0` (would
    /// produce an infinite loop).
    pub fn new(src_site: u16, dst_site: u16, tau_adv_ns: SimTime, link: LinkLatencyParams) -> Self {
        assert!(
            tau_adv_ns > 0,
            "AdvertisementChannel::new: tau_adv_ns must be > 0; got {tau_adv_ns}"
        );
        Self {
            src_site,
            dst_site,
            tau_adv_ns,
            link,
        }
    }
}

/// Compute the broadcast schedule for `channel` over the half-
/// open interval `[0, horizon_ns)`. The first broadcast fires at
/// `t = 0`; subsequent ones at `t = k · τ_adv` for `k > 0` while
/// `t < horizon_ns`.
///
/// `summary_at(now)` is invoked for every broadcast to capture
/// the source agent's snapshot at that simulator-time. The
/// function may be cheap (returning a constant) for tests, or
/// drive a real `ResourceAgent::advertised_summary` in
/// production.
pub fn schedule_broadcasts<F>(
    channel: AdvertisementChannel,
    horizon_ns: SimTime,
    mut summary_at: F,
    hierarchy: &RngHierarchy,
    replicate_index: u64,
) -> Vec<AdvertisementBroadcast>
where
    F: FnMut(SimTime) -> AdvertisedSummary,
{
    let replicate = hierarchy.replicate(replicate_index);
    // One stream per (src, dst) link — all broadcasts on the same
    // link draw sequentially from it. The T0.12 inventory lists
    // Stream::LinkLatency keyed by exactly this pair.
    let mut rng = replicate.stream(StreamId::LinkLatency {
        src_site: channel.src_site,
        dst_site: channel.dst_site,
    });

    let mut out = Vec::new();
    let mut t: SimTime = 0;
    while t < horizon_ns {
        let latency_ns =
            sample_lognormal_mean(&mut rng, channel.link.mean_ns, channel.link.log_sigma);
        let delivered = t.saturating_add(latency_ns.round() as SimTime);
        out.push(AdvertisementBroadcast {
            broadcast_at_ns: t,
            delivered_at_ns: delivered,
            summary: summary_at(t),
        });
        t = t.saturating_add(channel.tau_adv_ns);
    }
    out
}

/// Sample U ∈ (0, 1] from `rng`.
fn uniform_open_01(rng: &mut ChaCha20Rng) -> f64 {
    let bits = rng.next_u64() >> 11;
    let u = bits as f64 / (1u64 << 53) as f64;
    if u == 0.0 {
        f64::MIN_POSITIVE
    } else {
        u
    }
}

/// Standard-normal via Box-Muller.
fn sample_standard_normal(rng: &mut ChaCha20Rng) -> f64 {
    let u1 = uniform_open_01(rng);
    let u2 = uniform_open_01(rng);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

/// Lognormal sample with arithmetic mean = `target_mean`.
fn sample_lognormal_mean(rng: &mut ChaCha20Rng, target_mean: f64, sigma: f64) -> f64 {
    let z = sample_standard_normal(rng);
    let mu = target_mean.ln() - 0.5 * sigma * sigma;
    (mu + sigma * z).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(seed: u64) -> RngHierarchy {
        RngHierarchy::new(seed)
    }

    fn chan(tau: SimTime, mean_ns: f64, sigma: f64) -> AdvertisementChannel {
        AdvertisementChannel::new(0, 1, tau, LinkLatencyParams::new(mean_ns, sigma))
    }

    #[test]
    fn schedule_emits_broadcasts_at_tau_adv_cadence() {
        let tau = 1_000_000;
        let bcasts = schedule_broadcasts(
            chan(tau, 100_000.0, 0.0),
            5 * tau,
            |_| AdvertisedSummary::default(),
            &h(1),
            0,
        );
        assert_eq!(bcasts.len(), 5);
        for (i, b) in bcasts.iter().enumerate() {
            assert_eq!(b.broadcast_at_ns, i as SimTime * tau);
        }
    }

    #[test]
    fn zero_sigma_means_every_delivery_is_broadcast_plus_mean() {
        let tau = 1_000;
        let mean_ns = 250.0;
        let bcasts = schedule_broadcasts(
            chan(tau, mean_ns, 0.0),
            10 * tau,
            |_| AdvertisedSummary::default(),
            &h(2),
            0,
        );
        for b in &bcasts {
            assert_eq!(b.delivered_at_ns - b.broadcast_at_ns, mean_ns as SimTime);
        }
    }

    #[test]
    fn same_seed_same_replicate_yields_identical_schedule() {
        // Common-random-numbers smoke check: two invocations with
        // identical (hierarchy, replicate_index) emit the same
        // schedule. This is the foundation the strategy-
        // proofness replay test (Q2.3) relies on.
        let c = chan(1_000, 500.0, 0.3);
        let hierarchy = h(0xfeed);
        let a = schedule_broadcasts(c, 50_000, |_| AdvertisedSummary::default(), &hierarchy, 7);
        let b = schedule_broadcasts(c, 50_000, |_| AdvertisedSummary::default(), &hierarchy, 7);
        assert_eq!(a, b);
    }

    #[test]
    fn distinct_replicate_index_decorrelates_schedule() {
        let c = chan(1_000, 500.0, 0.3);
        let hierarchy = h(0xfeed);
        let a = schedule_broadcasts(c, 50_000, |_| AdvertisedSummary::default(), &hierarchy, 7);
        let b = schedule_broadcasts(c, 50_000, |_| AdvertisedSummary::default(), &hierarchy, 8);
        assert_ne!(a, b);
    }

    #[test]
    #[should_panic(expected = "tau_adv_ns must be > 0")]
    fn zero_tau_rejected() {
        AdvertisementChannel::new(0, 1, 0, LinkLatencyParams::new(1.0, 0.1));
    }

    #[test]
    #[should_panic(expected = "mean_ns must be > 0")]
    fn zero_mean_latency_rejected() {
        let _ = LinkLatencyParams::new(0.0, 0.1);
    }
}
