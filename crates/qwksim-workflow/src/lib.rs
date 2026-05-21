//! Workflow and DAG model for `qwksim`.
//!
//! Defines the `Task` (resource-demand vector + duration
//! distribution), the `Dag` (nodes + edges with category-aware data
//! volumes), and the `Workflow` envelope: `Category`, `min_iters`,
//! `max_iters`, `fidelity_class`, `deadline`, `priority`,
//! optional `modality_pref`, and the per-workflow declared
//! `convergence_threshold` driving the cv2 noisy-objective stopping
//! rule. Provides the six category templates (`Vqe`, `Qaoa`, `Qml`,
//! `QcMcQpe`, `PureClassical`, `Tomography`) with parameter envelopes
//! calibrated per Q11.3, and the iterative-loop runtime that
//! consults `Workflow::should_converge(...)` each iteration.
//!
//! See `plan/solution_plan.md` §2.4 and `plan/problem_definition.md`
//! §3 for the workflow specification.
