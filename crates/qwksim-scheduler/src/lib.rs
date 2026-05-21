//! Scheduler trait and dual-view abstraction for `qwksim`.
//!
//! Defines the single `Scheduler` trait implemented by every
//! mechanism (distributed cooperative bargainer, centralised
//! cooperative bargainer, HEFT, EASY-backfilling, FCFS), and the
//! two `View` types — `Oracular(&GlobalState)` and
//! `Local { local, advertised }` — that let the experiment harness
//! drive each mechanism under both information regimes (Q5.2 = Z,
//! 2×2 factorial). The trait surface is small on purpose:
//! `on_arrival(workflow, view)` and `on_event(event, view)`; the
//! mechanism owns its own internal state.
//!
//! See `plan/solution_plan.md` §2.6 for the trait sketch.
