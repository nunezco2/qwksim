//! DAG container and validator.
//!
//! A [`Dag`] is a `Vec<Task>` together with a `Vec<DagEdge>`
//! (parent → child plus the per-edge data volume). The validator
//! enforces three structural invariants:
//!
//! 1. Every edge's endpoints lie within `0..tasks.len()`.
//! 2. No self-loops (`from == to`).
//! 3. **Acyclicity** — checked by Kahn's algorithm (BFS topo
//!    sort). On failure, the cycle witness is returned in
//!    [`DagError::Cycle`].
//!
//! Source / sink uniqueness is **optional** (per the T2.7 issue
//! summary). The dedicated checker [`Dag::validate_strict`]
//! returns an error if more than one node has zero in-degree or
//! more than one has zero out-degree.
//!
//! Iteration order is deterministic at every step — `BTreeMap` /
//! `BTreeSet` only. The workspace `clippy::disallowed_methods`
//! gate (T1.11) backstops the choice.

use std::collections::{BTreeMap, BTreeSet};

use crate::task::Task;

/// Identifier for a task within a `Dag` — its index in the
/// `tasks` vector.
pub type TaskId = u32;

/// Directed edge in a `Dag`, carrying a per-edge data volume so
/// downstream phases can drive network and scratch I/O pools
/// from the same descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DagEdge {
    /// Producer task.
    pub from: TaskId,
    /// Consumer task.
    pub to: TaskId,
    /// Bytes transferred along this edge.
    pub data_volume_bytes: u64,
}

/// Acyclic task graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dag {
    /// Tasks in `TaskId` (= index) order.
    pub tasks: Vec<Task>,
    /// Edges; the validator enforces acyclicity but not edge
    /// uniqueness — multiple edges between the same `(from,
    /// to)` are legal (model two distinct data flows).
    pub edges: Vec<DagEdge>,
}

/// Reasons a [`Dag::validate`] call can fail.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DagError {
    /// An edge references a task index that does not exist.
    #[error("dag edge references unknown task {0}")]
    InvalidTaskId(TaskId),
    /// An edge is a self-loop (`from == to`).
    #[error("dag edge has self-loop on task {0}")]
    SelfLoop(TaskId),
    /// The dag contains a cycle. The associated `Vec<TaskId>`
    /// lists every task that belongs to a strongly-connected
    /// component of size ≥ 1 with a back edge — sorted by
    /// `TaskId` so the diagnostic is deterministic.
    #[error("dag has a cycle witnessed by tasks {0:?}")]
    Cycle(Vec<TaskId>),
    /// `validate_strict` only. The dag has more than one task
    /// with zero in-degree.
    #[error("dag has multiple source tasks {0:?}")]
    MultipleSources(Vec<TaskId>),
    /// `validate_strict` only. The dag has more than one task
    /// with zero out-degree.
    #[error("dag has multiple sink tasks {0:?}")]
    MultipleSinks(Vec<TaskId>),
    /// `validate_strict` only. The dag has zero tasks.
    #[error("dag is empty")]
    Empty,
}

impl Dag {
    /// Build a dag from a flat `Vec<Task>` and a flat
    /// `Vec<DagEdge>`. Does not validate; call [`Self::validate`]
    /// (or [`Self::validate_strict`]) before use.
    pub fn new(tasks: Vec<Task>, edges: Vec<DagEdge>) -> Self {
        Self { tasks, edges }
    }

    /// Convenience: empty dag (no tasks, no edges). Fails
    /// `validate_strict` but passes `validate`.
    pub fn empty() -> Self {
        Self {
            tasks: Vec::new(),
            edges: Vec::new(),
        }
    }

    /// Number of tasks.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// `true` iff the dag has no tasks.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Validate the structural invariants: every edge endpoint
    /// exists; no self-loops; the graph is acyclic.
    /// Source / sink uniqueness is **not** checked here — use
    /// [`Self::validate_strict`] for that.
    pub fn validate(&self) -> Result<(), DagError> {
        let n = self.tasks.len() as TaskId;

        // 1. Every edge's endpoints must be valid task ids.
        for edge in &self.edges {
            if edge.from >= n {
                return Err(DagError::InvalidTaskId(edge.from));
            }
            if edge.to >= n {
                return Err(DagError::InvalidTaskId(edge.to));
            }
            if edge.from == edge.to {
                return Err(DagError::SelfLoop(edge.from));
            }
        }

        // 2. Acyclicity via Kahn's BFS topological sort.
        let _ = self.kahn_topological_sort()?;
        Ok(())
    }

    /// Same as [`Self::validate`] plus:
    /// - exactly one task with zero in-degree (a unique source);
    /// - exactly one task with zero out-degree (a unique sink);
    /// - non-empty.
    pub fn validate_strict(&self) -> Result<(), DagError> {
        self.validate()?;
        if self.tasks.is_empty() {
            return Err(DagError::Empty);
        }
        let mut in_degree = self.in_degree_map();
        let mut out_degree = self.out_degree_map();
        let sources: Vec<TaskId> = in_degree
            .iter_mut()
            .filter(|(_, d)| **d == 0)
            .map(|(t, _)| *t)
            .collect();
        if sources.len() != 1 {
            return Err(DagError::MultipleSources(sources));
        }
        let sinks: Vec<TaskId> = out_degree
            .iter_mut()
            .filter(|(_, d)| **d == 0)
            .map(|(t, _)| *t)
            .collect();
        if sinks.len() != 1 {
            return Err(DagError::MultipleSinks(sinks));
        }
        Ok(())
    }

    /// Topological sort by Kahn's algorithm. Returns the
    /// task-id ordering when the graph is acyclic, or
    /// [`DagError::Cycle`] otherwise. Iteration over the active
    /// "ready" set uses `BTreeSet` so the returned order is
    /// deterministic under Q6′ = R2 (ties broken by ascending
    /// `TaskId`).
    pub fn topological_order(&self) -> Result<Vec<TaskId>, DagError> {
        // Endpoint validity must hold before topological sort.
        let n = self.tasks.len() as TaskId;
        for edge in &self.edges {
            if edge.from >= n {
                return Err(DagError::InvalidTaskId(edge.from));
            }
            if edge.to >= n {
                return Err(DagError::InvalidTaskId(edge.to));
            }
            if edge.from == edge.to {
                return Err(DagError::SelfLoop(edge.from));
            }
        }
        self.kahn_topological_sort()
    }

    fn kahn_topological_sort(&self) -> Result<Vec<TaskId>, DagError> {
        let n = self.tasks.len() as TaskId;
        let mut in_degree: BTreeMap<TaskId, usize> = (0..n).map(|t| (t, 0)).collect();
        let mut out_edges: BTreeMap<TaskId, Vec<TaskId>> = BTreeMap::new();
        for edge in &self.edges {
            *in_degree.entry(edge.to).or_insert(0) += 1;
            out_edges.entry(edge.from).or_default().push(edge.to);
        }

        let mut ready: BTreeSet<TaskId> = in_degree
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(t, _)| *t)
            .collect();
        let mut order = Vec::with_capacity(self.tasks.len());

        while let Some(&t) = ready.iter().next() {
            ready.remove(&t);
            order.push(t);
            if let Some(neighbours) = out_edges.get(&t) {
                for &n in neighbours {
                    let d = in_degree
                        .get_mut(&n)
                        .expect("in_degree initialised for every task");
                    *d -= 1;
                    if *d == 0 {
                        ready.insert(n);
                    }
                }
            }
        }

        if order.len() != self.tasks.len() {
            // Some tasks still have in-degree > 0 — they belong
            // to a cycle.
            let cycle_nodes: Vec<TaskId> = in_degree
                .iter()
                .filter(|(_, d)| **d > 0)
                .map(|(t, _)| *t)
                .collect();
            return Err(DagError::Cycle(cycle_nodes));
        }
        Ok(order)
    }

    fn in_degree_map(&self) -> BTreeMap<TaskId, usize> {
        let mut m: BTreeMap<TaskId, usize> =
            (0..self.tasks.len() as TaskId).map(|t| (t, 0)).collect();
        for edge in &self.edges {
            *m.entry(edge.to).or_insert(0) += 1;
        }
        m
    }

    fn out_degree_map(&self) -> BTreeMap<TaskId, usize> {
        let mut m: BTreeMap<TaskId, usize> =
            (0..self.tasks.len() as TaskId).map(|t| (t, 0)).collect();
        for edge in &self.edges {
            *m.entry(edge.from).or_insert(0) += 1;
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Task;

    fn t(d: u64) -> Task {
        Task::classical(d)
    }

    fn e(from: TaskId, to: TaskId) -> DagEdge {
        DagEdge {
            from,
            to,
            data_volume_bytes: 0,
        }
    }

    // ──────── valid DAGs ────────

    #[test]
    fn empty_dag_validates_but_strict_rejects() {
        let d = Dag::empty();
        assert!(d.validate().is_ok());
        assert_eq!(d.validate_strict(), Err(DagError::Empty));
    }

    #[test]
    fn single_node_dag_passes_validate_and_strict() {
        let d = Dag::new(vec![t(10)], vec![]);
        assert!(d.validate().is_ok());
        assert!(d.validate_strict().is_ok());
        assert_eq!(d.topological_order().unwrap(), vec![0]);
    }

    #[test]
    fn linear_pipeline_dag_validates_and_topo_sorts() {
        // 0 → 1 → 2 → 3
        let tasks = (0..4).map(|_| t(1)).collect();
        let edges = vec![e(0, 1), e(1, 2), e(2, 3)];
        let d = Dag::new(tasks, edges);
        assert!(d.validate().is_ok());
        assert!(d.validate_strict().is_ok());
        assert_eq!(d.topological_order().unwrap(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn fork_join_dag_validates() {
        // 0 → 1 ↘
        //          3
        // 0 → 2 ↗
        let d = Dag::new(
            (0..4).map(|_| t(1)).collect(),
            vec![e(0, 1), e(0, 2), e(1, 3), e(2, 3)],
        );
        assert!(d.validate().is_ok());
        assert!(d.validate_strict().is_ok());
        let order = d.topological_order().unwrap();
        // First task must be 0, last must be 3; 1 and 2 in
        // ascending order between them (BTreeSet tie-break).
        assert_eq!(order[0], 0);
        assert_eq!(order[3], 3);
        assert!(order.contains(&1));
        assert!(order.contains(&2));
    }

    #[test]
    fn dag_with_two_sources_passes_validate_but_fails_strict() {
        // 0 ↘
        //      2
        // 1 ↗
        let d = Dag::new((0..3).map(|_| t(1)).collect(), vec![e(0, 2), e(1, 2)]);
        assert!(d.validate().is_ok());
        match d.validate_strict() {
            Err(DagError::MultipleSources(s)) => assert_eq!(s, vec![0, 1]),
            other => panic!("expected MultipleSources, got {other:?}"),
        }
    }

    #[test]
    fn dag_with_two_sinks_passes_validate_but_fails_strict() {
        // 0 → 1
        //  ↓
        //  2
        let d = Dag::new((0..3).map(|_| t(1)).collect(), vec![e(0, 1), e(0, 2)]);
        assert!(d.validate().is_ok());
        match d.validate_strict() {
            Err(DagError::MultipleSinks(s)) => assert_eq!(s, vec![1, 2]),
            other => panic!("expected MultipleSinks, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_edges_are_legal() {
        // 0 → 1 (twice — modelling two distinct data flows)
        let d = Dag::new(vec![t(1), t(1)], vec![e(0, 1), e(0, 1)]);
        assert!(d.validate().is_ok());
    }

    // ──────── invalid DAGs ────────

    #[test]
    fn edge_to_unknown_task_id_rejected() {
        // 0 → 99 with only 2 tasks.
        let d = Dag::new(vec![t(1), t(1)], vec![e(0, 99)]);
        assert_eq!(d.validate(), Err(DagError::InvalidTaskId(99)));
    }

    #[test]
    fn edge_from_unknown_task_id_rejected() {
        let d = Dag::new(vec![t(1), t(1)], vec![e(99, 0)]);
        assert_eq!(d.validate(), Err(DagError::InvalidTaskId(99)));
    }

    #[test]
    fn self_loop_rejected() {
        let d = Dag::new(vec![t(1), t(1)], vec![e(0, 0)]);
        assert_eq!(d.validate(), Err(DagError::SelfLoop(0)));
    }

    #[test]
    fn two_node_cycle_rejected() {
        // 0 → 1 → 0
        let d = Dag::new(vec![t(1), t(1)], vec![e(0, 1), e(1, 0)]);
        match d.validate() {
            Err(DagError::Cycle(c)) => assert_eq!(c, vec![0, 1]),
            other => panic!("expected Cycle, got {other:?}"),
        }
    }

    #[test]
    fn three_node_cycle_rejected() {
        // 0 → 1 → 2 → 0
        let d = Dag::new(vec![t(1), t(1), t(1)], vec![e(0, 1), e(1, 2), e(2, 0)]);
        match d.validate() {
            Err(DagError::Cycle(c)) => assert_eq!(c, vec![0, 1, 2]),
            other => panic!("expected Cycle, got {other:?}"),
        }
    }

    #[test]
    fn cycle_only_in_subgraph_still_rejected() {
        // 0 → 1 (acyclic);  2 → 3 → 2 (cycle).
        let d = Dag::new(
            vec![t(1), t(1), t(1), t(1)],
            vec![e(0, 1), e(2, 3), e(3, 2)],
        );
        match d.validate() {
            Err(DagError::Cycle(c)) => assert_eq!(c, vec![2, 3]),
            other => panic!("expected Cycle, got {other:?}"),
        }
    }

    #[test]
    fn topological_order_on_cycle_returns_cycle_error() {
        let d = Dag::new(vec![t(1), t(1)], vec![e(0, 1), e(1, 0)]);
        assert!(matches!(d.topological_order(), Err(DagError::Cycle(_))));
    }

    // ──────── builders ────────

    #[test]
    fn empty_constructors_match_is_empty_and_len() {
        let d = Dag::empty();
        assert_eq!(d.len(), 0);
        assert!(d.is_empty());
        let d2 = Dag::new(vec![t(1)], vec![]);
        assert_eq!(d2.len(), 1);
        assert!(!d2.is_empty());
    }
}
