//! `Workflow` envelope (Q8.1 / §2.4 / §5.2).
//!
//! Bundles a [`Dag`](crate::Dag) of [`Task`](crate::Task)s with
//! the iterative-loop bracket (`min_iters`, `max_iters`) plus
//! per-workflow metadata (category, identifier). Concrete
//! per-category templates (`Vqe`, `Qaoa`, …) land in sibling
//! modules under this crate; each emits a `Workflow` whose
//! aggregate expected wallclock matches the Q11.3 anchor for the
//! category.
//!
//! `Workflow` is intentionally lean today — the fidelity-class
//! / deadline / priority fields from the questionnaire's
//! workflow envelope land alongside the Phase-2 PRs that
//! produce them (T2.10 attributes, etc.). T2.9 lands only what
//! the VQE template needs.

use crate::dag::Dag;

/// Workflow category enumeration matching Q8.6's 6-category mix.
///
/// All categories will land Phase 2:
/// - **`Vqe`** — T2.9 (this PR).
/// - **`Qaoa`** — T2.10+.
/// - **`Qml`** — T2.x.
/// - **`QcMcQpe`** — Quantum-classical Monte Carlo / phase
///   estimation; T2.x.
/// - **`PureClassical`** — Q8.6 (5); T2.x.
/// - **`Tomography`** — Quantum error-mitigation tomography; T2.x.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Vqe,
    Qaoa,
    Qml,
    QcMcQpe,
    PureClassical,
    Tomography,
}

/// One workflow instance — a category-tagged [`Dag`] with an
/// iterative-loop bracket.
///
/// Cheap to clone (`Dag` clones its `Vec<Task>` / `Vec<DagEdge>`),
/// but for the Phase-2 simulator path workflows are typically
/// constructed once per arrival and consumed by the scheduler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    /// Per-replicate-unique identifier.
    pub workflow_id: u64,
    /// Category tag.
    pub category: Category,
    /// Task graph.
    pub dag: Dag,
    /// Minimum iterations before the iterative-loop runtime is
    /// allowed to converge. Matches the cv2 spec from T2.8.
    pub min_iters: u32,
    /// Maximum iterations the runtime will execute before
    /// forcing termination.
    pub max_iters: u32,
}

impl Workflow {
    /// Midpoint of `[min_iters, max_iters]` — the first-pass
    /// expected iteration count used in
    /// [`Self::expected_wallclock_ns`]. Concrete distributions
    /// land in T2.8's runtime once the simulator threads the
    /// noisy-objective state through. Returns `f64` because
    /// downstream arithmetic mixes it with `duration_ns` sums
    /// that can grow large.
    pub fn expected_iters(&self) -> f64 {
        (self.min_iters as f64 + self.max_iters as f64) * 0.5
    }

    /// Expected total simulator wallclock for one run of this
    /// workflow: `sum(task durations) × expected_iters`. Used by
    /// the category-template tests (T2.9 acceptance gate).
    pub fn expected_wallclock_ns(&self) -> f64 {
        let one_iter: u64 = self.dag.tasks.iter().map(|t| t.duration_ns).sum();
        one_iter as f64 * self.expected_iters()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::{Dag, DagEdge};
    use crate::task::Task;

    fn lin(ns: &[u64]) -> Dag {
        let tasks: Vec<Task> = ns.iter().map(|&d| Task::classical(d)).collect();
        let edges: Vec<DagEdge> = (0..ns.len().saturating_sub(1) as u32)
            .map(|i| DagEdge {
                from: i,
                to: i + 1,
                data_volume_bytes: 0,
            })
            .collect();
        Dag::new(tasks, edges)
    }

    #[test]
    fn expected_iters_is_the_midpoint() {
        let w = Workflow {
            workflow_id: 0,
            category: Category::Vqe,
            dag: lin(&[0]),
            min_iters: 10,
            max_iters: 50,
        };
        assert_eq!(w.expected_iters(), 30.0);
    }

    #[test]
    fn expected_wallclock_is_sum_of_durations_times_expected_iters() {
        // Three tasks at 1 / 2 / 3 ns; min/max 4/8 → midpoint 6.
        let w = Workflow {
            workflow_id: 0,
            category: Category::Vqe,
            dag: lin(&[1, 2, 3]),
            min_iters: 4,
            max_iters: 8,
        };
        // (1 + 2 + 3) × 6 = 36
        assert_eq!(w.expected_wallclock_ns(), 36.0);
    }
}
