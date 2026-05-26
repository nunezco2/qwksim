//! Integration test for **T2.12** — c2 analytical-validation
//! gate (fork-join makespan) from §12 of the engineering plan.
//!
//! Implements the canonical greedy **list-scheduling** algorithm
//! over an arbitrary [`Dag`] on `k` identical cores:
//!
//! 1. Visit tasks in topological order.
//! 2. For each task, its earliest enable time is the `max` of
//!    its predecessors' completion times.
//! 3. Pick the core whose `busy_until` is smallest among those
//!    that are free by the enable time; ties broken by ascending
//!    core index.
//! 4. The task's actual start is `max(enable_time,
//!    busy_until[chosen])`; completion = start + duration.
//!
//! For balanced fork-join DAGs (`source → N parallel branches →
//! sink`) the closed-form list-scheduling makespan is
//!
//! ```text
//! makespan = duration(source)
//!          + ceil(N / k) * duration(branch)
//!          + duration(sink)
//! ```
//!
//! The integration test exercises hand-constructed cases against
//! this formula. The HEFT scheduler in T5.x will then be
//! validated against the same list-scheduling helper (proving
//! HEFT is **at least as good** as greedy list-scheduling on
//! every DAG in the test set — that's the c3 follow-on gate).

use std::collections::BTreeMap;

use qwksim_core::event::SimTime;
use qwksim_workflow::dag::{Dag, DagEdge, TaskId};
use qwksim_workflow::task::Task;

/// Greedy list-scheduling: walk `dag` in topological order;
/// dispatch each task to the earliest-free core. Returns the
/// schedule's makespan.
fn list_schedule_makespan(dag: &Dag, cores: u32) -> SimTime {
    assert!(cores > 0, "cores must be > 0; got {cores}");
    let order = dag.topological_order().expect("dag must be acyclic");
    let mut busy_until: Vec<SimTime> = vec![0; cores as usize];
    let mut completion: BTreeMap<TaskId, SimTime> = BTreeMap::new();

    for &task_id in &order {
        // Earliest enable time: latest predecessor completion.
        let preds: Vec<TaskId> = dag
            .edges
            .iter()
            .filter(|e| e.to == task_id)
            .map(|e| e.from)
            .collect();
        let enable_time = preds
            .iter()
            .map(|p| *completion.get(p).expect("predecessor completed first"))
            .max()
            .unwrap_or(0);

        // Pick the core with smallest busy_until, ties broken by
        // ascending index (BTreeMap iteration semantics in the
        // resolved-state world; the explicit `enumerate` here is
        // deterministic by construction).
        let (chosen_core, chosen_busy) = busy_until
            .iter()
            .enumerate()
            .min_by_key(|(idx, &b)| (b, *idx))
            .map(|(idx, b)| (idx, *b))
            .expect("non-zero cores");
        let actual_start = enable_time.max(chosen_busy);
        let task = &dag.tasks[task_id as usize];
        let actual_completion = actual_start.saturating_add(task.duration_ns);
        busy_until[chosen_core] = actual_completion;
        completion.insert(task_id, actual_completion);
    }

    *completion.values().max().expect("non-empty dag")
}

/// `source → N parallel branches → sink`. Returns a Dag with
/// `2 + N` tasks: task 0 = source, tasks `1..=N` = branches,
/// task `N + 1` = sink.
fn fork_join(source_ns: u64, branch_ns: u64, sink_ns: u64, n_branches: u32) -> Dag {
    let mut tasks: Vec<Task> = Vec::with_capacity(2 + n_branches as usize);
    tasks.push(Task::classical(source_ns));
    for _ in 0..n_branches {
        tasks.push(Task::classical(branch_ns));
    }
    tasks.push(Task::classical(sink_ns));

    let source = 0u32;
    let sink = (n_branches + 1) as TaskId;
    let mut edges = Vec::with_capacity(2 * n_branches as usize);
    for i in 0..n_branches {
        let branch = (i + 1) as TaskId;
        edges.push(DagEdge {
            from: source,
            to: branch,
            data_volume_bytes: 0,
        });
        edges.push(DagEdge {
            from: branch,
            to: sink,
            data_volume_bytes: 0,
        });
    }
    Dag::new(tasks, edges)
}

/// Closed-form list-scheduling makespan for a balanced fork-join
/// DAG. Used to cross-check the algorithmic output for any
/// `(source, branch, sink, n, k)` tuple.
fn analytical_fork_join_makespan(
    source_ns: u64,
    branch_ns: u64,
    sink_ns: u64,
    n_branches: u32,
    cores: u32,
) -> SimTime {
    let batches = n_branches.div_ceil(cores);
    source_ns + batches as u64 * branch_ns + sink_ns
}

#[test]
fn four_branch_fork_join_with_four_cores_runs_branches_in_parallel() {
    let dag = fork_join(
        /* source */ 1, /* branch */ 5, /* sink */ 1, 4,
    );
    let observed = list_schedule_makespan(&dag, 4);
    let analytical = analytical_fork_join_makespan(1, 5, 1, 4, 4);
    assert_eq!(observed, analytical);
    assert_eq!(observed, 7);
}

#[test]
fn four_branch_fork_join_with_two_cores_runs_branches_in_two_batches() {
    let dag = fork_join(1, 5, 1, 4);
    let observed = list_schedule_makespan(&dag, 2);
    let analytical = analytical_fork_join_makespan(1, 5, 1, 4, 2);
    assert_eq!(observed, analytical);
    assert_eq!(observed, 12);
}

#[test]
fn four_branch_fork_join_with_one_core_serialises_completely() {
    let dag = fork_join(1, 5, 1, 4);
    let observed = list_schedule_makespan(&dag, 1);
    let analytical = analytical_fork_join_makespan(1, 5, 1, 4, 1);
    assert_eq!(observed, analytical);
    assert_eq!(observed, 22);
}

#[test]
fn ten_branch_fork_join_with_three_cores_follows_ceil_division() {
    // source 2 → 10 branches × 3 → sink 2, on 3 cores.
    // ceil(10/3) = 4 batches of 3 ms each.
    // makespan = 2 + 4×3 + 2 = 16.
    let dag = fork_join(2, 3, 2, 10);
    let observed = list_schedule_makespan(&dag, 3);
    let analytical = analytical_fork_join_makespan(2, 3, 2, 10, 3);
    assert_eq!(observed, analytical);
    assert_eq!(observed, 16);
}

#[test]
fn single_branch_fork_join_collapses_to_pipeline() {
    // source → 1 branch → sink. Any core count: 2 + 5 + 3 = 10.
    let dag = fork_join(2, 5, 3, 1);
    for cores in [1u32, 2, 4, 8] {
        let observed = list_schedule_makespan(&dag, cores);
        assert_eq!(
            observed, 10,
            "1-branch chain must collapse to source + branch + sink = 10 regardless of cores ({cores})"
        );
    }
}

#[test]
fn list_scheduler_handles_a_dozen_uneven_fork_join_sweeps() {
    // Sweep over (n_branches, cores) and verify every case
    // against the closed-form analytical formula. Sweep is
    // deterministic and small (≤ 36 cases); runtime is
    // dominated by Dag construction, well under the 30-second
    // budget.
    for &n in &[1u32, 2, 3, 4, 6, 12] {
        for &k in &[1u32, 2, 3, 6] {
            let dag = fork_join(/* src */ 1, /* branch */ 7, /* sink */ 1, n);
            let observed = list_schedule_makespan(&dag, k);
            let analytical = analytical_fork_join_makespan(1, 7, 1, n, k);
            assert_eq!(
                observed, analytical,
                "n={n}, k={k}: observed {observed} ≠ analytical {analytical}",
            );
        }
    }
}
