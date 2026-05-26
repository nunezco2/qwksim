//! VQE (Variational Quantum Eigensolver) category template per
//! Q11.3 anchors:
//!
//! - **8 base tasks** in a linear pipeline (one DAG per workflow
//!   instance; the iterative-loop runtime wraps the whole DAG
//!   `[min_iters, max_iters]` times).
//! - `min_iters = 10`, `max_iters = 50` → midpoint
//!   **expected_iters = 30**.
//! - Per-task duration drawn from a **log-normal** distribution
//!   with the anchor as the *mean* (not the median). With 30
//!   expected iterations and 8 tasks at a 1.5 s mean, expected
//!   wallclock = `8 × 1.5 s × 30 = 360 s ≈ 6 min`, matching the
//!   Q11.3 anchor.
//! - Per-task duration draws use the
//!   `Stream::TaskDuration(workflow_id, task_index)` ChaCha20
//!   stream from T0.12 — the calling simulator passes a
//!   [`qwksim_core::rng::RngHierarchy`] and a `replicate_index`
//!   so the draws are reproducible under Q6′ = R2 deterministic
//!   replay.
//!
//! Per Q8.6 the VQE workflow's tasks are *iterative*: every
//! task runs once per iteration of the outer loop. The
//! template-level DAG is therefore a single `(task_0, …, task_7)`
//! linear pipeline; the loop wrapper lives in `qwksim_workflow::iter`.

use qwksim_core::rng::{RngHierarchy, StreamId};
use rand_chacha::ChaCha20Rng;
use rand_core::RngCore;

use crate::dag::{Dag, DagEdge, TaskId};
use crate::task::Task;
use crate::workflow::{Category, Workflow};

/// Configurable parameters for the VQE template.
///
/// All fields default to the Q11.3 anchors via `Default`; the
/// public constructor is [`VqeConfig::for_workflow`] which only
/// requires the per-workflow identifier.
#[derive(Debug, Clone, Copy)]
pub struct VqeConfig {
    /// Per-replicate-unique workflow identifier.
    pub workflow_id: u64,
    /// Number of tasks in the per-iteration DAG (Q11.3: 8).
    pub base_task_count: u32,
    /// Minimum iterations (Q11.3: 10).
    pub min_iters: u32,
    /// Maximum iterations (Q11.3: 50).
    pub max_iters: u32,
    /// Mean per-task wallclock (simulator nanoseconds). Chosen
    /// so `base_task_count × per_task_mean × expected_iters` ≈ 6
    /// minutes per Q11.3 — default `1_500_000_000 ns` (1.5 s)
    /// gives `8 × 1.5 s × 30 = 360 s`.
    pub per_task_mean_ns: u64,
    /// Standard deviation of the **log-normal** distribution
    /// (in log-space). Default `0.2`.
    pub duration_log_sigma: f64,
    /// Bytes transferred on each pipeline edge. Default 1 KiB
    /// (small; quantum-segment edges carry mostly metadata).
    pub edge_data_volume_bytes: u64,
}

impl VqeConfig {
    /// Build a `VqeConfig` for `workflow_id` with every other
    /// field set to its Q11.3 anchor default.
    pub fn for_workflow(workflow_id: u64) -> Self {
        Self {
            workflow_id,
            base_task_count: 8,
            min_iters: 10,
            max_iters: 50,
            per_task_mean_ns: 1_500_000_000,
            duration_log_sigma: 0.2,
            edge_data_volume_bytes: 1024,
        }
    }
}

/// Produce a parameterised VQE [`Workflow`].
///
/// Per-task durations are drawn from the
/// `Stream::TaskDuration(workflow_id, task_index)` stream so
/// repeated calls with the same `(hierarchy, replicate_index,
/// workflow_id)` triple return bit-identical workflows
/// (Q6′ = R2).
pub fn template(cfg: VqeConfig, hierarchy: &RngHierarchy, replicate_index: u64) -> Workflow {
    assert!(
        cfg.base_task_count > 0,
        "VqeConfig::base_task_count must be > 0; got {}",
        cfg.base_task_count
    );
    assert!(
        cfg.min_iters <= cfg.max_iters && cfg.max_iters > 0,
        "VqeConfig requires 0 < max_iters and min_iters ≤ max_iters; got min={}, max={}",
        cfg.min_iters,
        cfg.max_iters,
    );
    assert!(
        cfg.duration_log_sigma >= 0.0 && cfg.duration_log_sigma.is_finite(),
        "VqeConfig::duration_log_sigma must be ≥ 0 and finite; got {}",
        cfg.duration_log_sigma,
    );

    let replicate = hierarchy.replicate(replicate_index);

    let mut tasks: Vec<Task> = Vec::with_capacity(cfg.base_task_count as usize);
    for task_index in 0..cfg.base_task_count {
        let mut rng = replicate.stream(StreamId::TaskDuration {
            workflow_id: cfg.workflow_id,
            task_index,
        });
        let duration_ns = sample_lognormal_mean(
            &mut rng,
            cfg.per_task_mean_ns as f64,
            cfg.duration_log_sigma,
        );
        tasks.push(Task::classical(duration_ns.round() as u64));
    }

    let edges: Vec<DagEdge> = (0..(cfg.base_task_count.saturating_sub(1)))
        .map(|i| DagEdge {
            from: i as TaskId,
            to: (i + 1) as TaskId,
            data_volume_bytes: cfg.edge_data_volume_bytes,
        })
        .collect();

    Workflow {
        workflow_id: cfg.workflow_id,
        category: Category::Vqe,
        dag: Dag::new(tasks, edges),
        min_iters: cfg.min_iters,
        max_iters: cfg.max_iters,
    }
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

/// Sample one standard-normal variate via Box-Muller.
fn sample_standard_normal(rng: &mut ChaCha20Rng) -> f64 {
    let u1 = uniform_open_01(rng);
    let u2 = uniform_open_01(rng);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

/// Sample from a log-normal whose **arithmetic mean** equals
/// `target_mean`. For log-normal with parameters `(μ, σ)`:
/// `mean = exp(μ + σ² / 2)`, so to centre the mean at
/// `target_mean` we set `μ = ln(target_mean) − σ² / 2`.
fn sample_lognormal_mean(rng: &mut ChaCha20Rng, target_mean: f64, sigma: f64) -> f64 {
    let z = sample_standard_normal(rng);
    let mu = target_mean.ln() - 0.5 * sigma * sigma;
    (mu + sigma * z).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hierarchy(seed: u64) -> RngHierarchy {
        RngHierarchy::new(seed)
    }

    #[test]
    fn default_config_carries_q11_3_anchors() {
        let cfg = VqeConfig::for_workflow(42);
        assert_eq!(cfg.workflow_id, 42);
        assert_eq!(cfg.base_task_count, 8);
        assert_eq!(cfg.min_iters, 10);
        assert_eq!(cfg.max_iters, 50);
        assert_eq!(cfg.per_task_mean_ns, 1_500_000_000);
        assert_eq!(cfg.duration_log_sigma, 0.2);
    }

    #[test]
    fn template_emits_a_linear_pipeline_dag() {
        let w = template(VqeConfig::for_workflow(0), &hierarchy(1), 0);
        assert_eq!(w.category, Category::Vqe);
        assert_eq!(w.dag.tasks.len(), 8);
        assert_eq!(w.dag.edges.len(), 7);
        // Linear pipeline: every edge is (i, i+1).
        for (i, edge) in w.dag.edges.iter().enumerate() {
            assert_eq!(edge.from, i as TaskId);
            assert_eq!(edge.to, (i + 1) as TaskId);
            assert_eq!(edge.data_volume_bytes, 1024);
        }
        assert!(w.dag.validate().is_ok());
        assert!(w.dag.validate_strict().is_ok());
    }

    #[test]
    fn template_is_deterministic_under_same_inputs() {
        let h = hierarchy(7);
        let a = template(VqeConfig::for_workflow(1), &h, 100);
        let b = template(VqeConfig::for_workflow(1), &h, 100);
        assert_eq!(a, b);
    }

    #[test]
    fn template_decorrelates_under_distinct_replicate_index() {
        let h = hierarchy(7);
        let a = template(VqeConfig::for_workflow(1), &h, 100);
        let b = template(VqeConfig::for_workflow(1), &h, 101);
        // Per-task durations are independent samples; near-zero
        // probability of exact match across 8 floats.
        assert_ne!(a, b);
    }

    #[test]
    fn template_decorrelates_under_distinct_workflow_id() {
        let h = hierarchy(7);
        let a = template(VqeConfig::for_workflow(1), &h, 100);
        let b = template(VqeConfig::for_workflow(2), &h, 100);
        assert_ne!(a, b);
    }

    #[test]
    fn one_thousand_sampled_workflows_have_mean_wallclock_near_q11_3_anchor() {
        // T2.9 headline acceptance gate. The Q11.3 anchor wallclock
        // for a VQE workflow is:
        //   base_tasks × per_task_mean × expected_iters
        //   = 8 × 1.5 s × ((10 + 50) / 2)
        //   = 8 × 1.5 s × 30
        //   = 360 s = 360_000_000_000 ns.
        //
        // The lognormal-mean construction in sample_lognormal_mean
        // sets exp(μ + σ²/2) = target_mean so the per-task expected
        // value matches the anchor exactly. Across 1000 sampled
        // workflows × 8 tasks = 8000 samples, the sample-mean
        // precision is comfortably below the 5 % tolerance asserted
        // here.
        let h = hierarchy(0xdead_beef);
        let target_wallclock_ns = 360_000_000_000.0; // 6 min in ns
        const N: usize = 1000;
        let mut sum_wallclock = 0.0f64;
        for replicate in 0..N as u64 {
            // Vary workflow_id with replicate so the underlying
            // streams are independent across samples.
            let w = template(VqeConfig::for_workflow(replicate), &h, replicate);
            sum_wallclock += w.expected_wallclock_ns();
        }
        let mean_wallclock = sum_wallclock / N as f64;
        let rel_err = (mean_wallclock - target_wallclock_ns).abs() / target_wallclock_ns;
        assert!(
            rel_err < 0.05,
            "mean wallclock = {mean_wallclock} ns differs from anchor {target_wallclock_ns} ns by {} %",
            rel_err * 100.0,
        );
    }

    #[test]
    #[should_panic(expected = "base_task_count")]
    fn zero_base_task_count_rejected() {
        let cfg = VqeConfig {
            base_task_count: 0,
            ..VqeConfig::for_workflow(0)
        };
        let _ = template(cfg, &hierarchy(0), 0);
    }

    #[test]
    #[should_panic(expected = "min_iters")]
    fn min_iters_gt_max_iters_rejected() {
        let cfg = VqeConfig {
            min_iters: 100,
            max_iters: 50,
            ..VqeConfig::for_workflow(0)
        };
        let _ = template(cfg, &hierarchy(0), 0);
    }

    #[test]
    #[should_panic(expected = "duration_log_sigma")]
    fn negative_sigma_rejected() {
        let cfg = VqeConfig {
            duration_log_sigma: -0.1,
            ..VqeConfig::for_workflow(0)
        };
        let _ = template(cfg, &hierarchy(0), 0);
    }
}
