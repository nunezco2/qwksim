//! Baseline schedulers for `qwksim`.
//!
//! Hosts the four reference schedulers compared against the
//! distributed cooperative bargainer in the headline 2×2 factorial:
//!
//! - [`Fcfs`] — first-come-first-served single global queue (no-knowledge control).
//! - `EasyBackfill` — classic SLURM-style EASY backfilling (lands in T5.x).
//! - `Heft` — Heterogeneous Earliest Finish Time list scheduling for DAGs (T5.x).
//! - `CentralisedCooperativeBargainer` — the headline comparator
//!   (FLAG-G closed): invokes the `qwksim-bargain` solver with a
//!   global view and a single Nash product over every super-site's
//!   resource agents (lands in T5.3).
//!
//! Each baseline implements the [`qwksim_scheduler::Scheduler`]
//! trait and runs under both the `Oracular` and `Local` views.
//!
//! See `plan/solution_plan.md` §2.7 and `plan/problem_definition.md`
//! §5 for the baseline rationale.

pub mod fcfs;

pub use fcfs::Fcfs;
