//! Integration test for the **T1.5** acceptance criterion: a
//! trivial single-resource scheduler runs identically under both
//! `View` variants on degenerate input.
//!
//! "Degenerate input" here is the same sequence of arrivals and
//! events fed to two independent `EchoScheduler` instances — one
//! driven via [`View::Oracular`], the other via [`View::Local`].
//! The two final scheduler states must be `==`, because an
//! information-regime-blind scheduler must by construction not
//! observe any difference between the two regimes.

use qwksim_core::event::{Event, EventKind};
use qwksim_scheduler::{
    AdvertisedState, EchoScheduler, GlobalState, LocalState, Scheduler, View, Workflow,
};

fn arrival_event(at: u64, agent: u32, seq: u64) -> Event {
    Event {
        at,
        agent,
        seq,
        kind: EventKind::Arrival,
    }
}

#[test]
fn trivial_scheduler_runs_identically_under_both_views() {
    // Same input stream fed to both schedulers.
    let workflows = [Workflow; 3];
    let events = [
        arrival_event(10, 0, 0),
        arrival_event(20, 1, 1),
        arrival_event(30, 0, 2),
        arrival_event(40, 2, 3),
    ];

    // Backing state values for the two view variants.
    let g = GlobalState;
    let l = LocalState;
    let a = AdvertisedState;

    // Scheduler instance driven under the Oracular view.
    let mut oracular_sched = EchoScheduler::default();
    let oracular_view = || View::Oracular(&g);
    for w in &workflows {
        oracular_sched.on_arrival(w, oracular_view());
    }
    for e in &events {
        oracular_sched.on_event(e, oracular_view());
    }

    // Scheduler instance driven under the Local view.
    let mut local_sched = EchoScheduler::default();
    let local_view = || View::Local {
        local: &l,
        advertised: &a,
    };
    for w in &workflows {
        local_sched.on_arrival(w, local_view());
    }
    for e in &events {
        local_sched.on_event(e, local_view());
    }

    assert_eq!(
        oracular_sched, local_sched,
        "an information-regime-blind scheduler must observe identical state under both views"
    );
    assert_eq!(
        oracular_sched.arrivals,
        workflows.len() as u32,
        "every workflow arrival reaches the scheduler exactly once"
    );
    assert_eq!(
        oracular_sched.events,
        events.len() as u32,
        "every event reaches the scheduler exactly once"
    );
}

#[test]
fn view_variants_round_trip_through_trait_dispatch() {
    // Sanity: dispatching through the trait does not silently
    // collapse the two view variants. We feed an arrival under
    // each view in turn and confirm the scheduler still sees the
    // right variant inside its on_arrival via a small probe.

    #[derive(Default)]
    struct Probe {
        last_was_oracular: Option<bool>,
    }

    impl Scheduler for Probe {
        fn on_arrival(
            &mut self,
            _workflow: &Workflow,
            view: View<'_>,
        ) -> qwksim_scheduler::SchedulingDecision {
            self.last_was_oracular = Some(view.is_oracular());
            qwksim_scheduler::SchedulingDecision
        }

        fn on_event(&mut self, _event: &Event, _view: View<'_>) {}
    }

    let g = GlobalState;
    let l = LocalState;
    let a = AdvertisedState;
    let mut probe = Probe::default();

    probe.on_arrival(&Workflow, View::Oracular(&g));
    assert_eq!(probe.last_was_oracular, Some(true));

    probe.on_arrival(
        &Workflow,
        View::Local {
            local: &l,
            advertised: &a,
        },
    );
    assert_eq!(probe.last_was_oracular, Some(false));
}
