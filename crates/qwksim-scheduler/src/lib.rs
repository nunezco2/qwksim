//! Scheduler trait and dual-view abstraction for `qwksim`.
//!
//! Defines the single [`Scheduler`] trait implemented by every
//! mechanism (distributed cooperative bargainer, centralised
//! cooperative bargainer, HEFT, EASY-backfilling, FCFS) and the two
//! [`View`] variants — `Oracular(&GlobalState)` and `Local { local,
//! advertised }` — that let the experiment harness drive each
//! mechanism under both information regimes (Q5.2 = Z, 2×2
//! factorial isolating *mechanism* from *information*).
//!
//! The trait surface is small on purpose:
//!
//! - [`Scheduler::on_arrival`] is called when the federation router
//!   admits a new workflow.
//! - [`Scheduler::on_event`] is called for every other simulator
//!   event the scheduler subscribes to (advertise, bargain-round,
//!   completion, …).
//!
//! Both methods take a [`View<'_>`]; the implementation is free to
//! match on the view variant or ignore it. A scheduler that ignores
//! the view (e.g. a trivial FCFS) must behave identically under
//! both — exactly what the T1.5 integration test asserts.
//!
//! See `plan/solution_plan.md` §2.6 for the trait sketch and §6 for
//! the dual-view rationale.

use qwksim_core::event::Event;

/// Placeholder for the federation-wide state visible under the
/// **oracular** information regime.
///
/// Concrete fields land alongside the per-resource agents
/// (Phase 2) and the bargaining solver (Phase 4). Today this is a
/// marker type so the [`View`] enum compiles and the dual-view
/// abstraction can be exercised end-to-end on degenerate input.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlobalState;

/// Placeholder for an agent's local view of its own state, used in
/// the **local + advertised** information regime.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalState;

/// Placeholder for the τ_adv-stale advertised-summary state an
/// agent sees about its peers, used in the **local + advertised**
/// regime.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AdvertisedState;

/// Placeholder workflow handle. The full workflow schema lands in
/// `qwksim-workflow` (Phase 2); the scheduler trait only needs a
/// `&Workflow` reference to route arrivals.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Workflow;

/// Placeholder for the scheduling decision returned from
/// [`Scheduler::on_arrival`]. Concrete fields (chosen site, chosen
/// agent, bargaining envelope, …) land in later phases.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SchedulingDecision;

/// The information regime under which a scheduler is invoked.
///
/// - [`View::Oracular`] — full global state visible. This is the
///   regime under which the **centralised** baselines (HEFT, EASY,
///   FCFS, centralised cooperative bargainer) run, plus the upper
///   half of the 2×2 factorial for the distributed bargainer.
/// - [`View::Local`] — only the agent's own local state plus
///   advertised summaries from its peers. This is the realistic
///   regime under which the **distributed** cooperative bargainer
///   runs, plus the lower half of the 2×2 factorial for centralised
///   baselines (to isolate mechanism from information).
#[derive(Debug, Clone, Copy)]
pub enum View<'a> {
    /// Full global-state view; no advertisement staleness.
    Oracular(&'a GlobalState),
    /// Local + τ_adv-stale advertised summaries.
    Local {
        /// The agent's own state.
        local: &'a LocalState,
        /// The summary the agent currently sees about its peers.
        advertised: &'a AdvertisedState,
    },
}

impl View<'_> {
    /// `true` iff this is an `Oracular` view.
    pub fn is_oracular(&self) -> bool {
        matches!(self, View::Oracular(_))
    }

    /// `true` iff this is a `Local` view.
    pub fn is_local(&self) -> bool {
        matches!(self, View::Local { .. })
    }
}

/// Common interface every mechanism implements.
///
/// `&mut self` is mandatory: real implementations carry internal
/// state (queues, cached views, bargaining-round state, …) that
/// mutates with every call.
pub trait Scheduler {
    /// Handle a new workflow arriving at the federation router.
    fn on_arrival(&mut self, workflow: &Workflow, view: View<'_>) -> SchedulingDecision;

    /// Handle any other simulator event the scheduler subscribes
    /// to (advertise tick, completion, bargain-round trigger, …).
    fn on_event(&mut self, event: &Event, view: View<'_>);
}

/// Trivial test-fixture scheduler. Counts the number of calls to
/// each trait method without looking at the view at all — so its
/// observable behaviour is necessarily identical under
/// [`View::Oracular`] and [`View::Local`]. T1.5's integration test
/// asserts exactly that property.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EchoScheduler {
    /// How many times [`Scheduler::on_arrival`] has been called.
    pub arrivals: u32,
    /// How many times [`Scheduler::on_event`] has been called.
    pub events: u32,
}

impl Scheduler for EchoScheduler {
    fn on_arrival(&mut self, _workflow: &Workflow, _view: View<'_>) -> SchedulingDecision {
        self.arrivals += 1;
        SchedulingDecision
    }

    fn on_event(&mut self, _event: &Event, _view: View<'_>) {
        self.events += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwksim_core::event::{Event, EventKind};

    fn ev(at: u64, agent: u32, seq: u64) -> Event {
        Event {
            at,
            agent,
            seq,
            kind: EventKind::Arrival,
        }
    }

    #[test]
    fn view_variants_are_discriminable() {
        let g = GlobalState;
        let l = LocalState;
        let a = AdvertisedState;
        let oracular = View::Oracular(&g);
        let local = View::Local {
            local: &l,
            advertised: &a,
        };
        assert!(oracular.is_oracular());
        assert!(!oracular.is_local());
        assert!(!local.is_oracular());
        assert!(local.is_local());
    }

    #[test]
    fn echo_scheduler_counts_each_call_once() {
        let mut s = EchoScheduler::default();
        let g = GlobalState;
        s.on_arrival(&Workflow, View::Oracular(&g));
        s.on_event(&ev(0, 0, 0), View::Oracular(&g));
        assert_eq!(s.arrivals, 1);
        assert_eq!(s.events, 1);
    }
}
