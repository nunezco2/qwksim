//! Integration test for the **T1.3** acceptance criterion: an agent
//! that `.await`s a `DesSleep` of 100 ns wakes at simulator time
//! 100 ns exactly, with effectively zero wall-clock delay.

use std::time::Instant;

use qwksim_core::sim::Simulator;

#[test]
fn des_sleep_100ns_wakes_at_sim_time_100_with_no_wall_clock_delay() {
    let sim = Simulator::new();
    let handle = sim.handle();

    let start = Instant::now();
    sim.run(move |h| async move {
        assert_eq!(h.now(), 0, "agent starts at simulator time 0");
        h.sleep_for(100).await;
        assert_eq!(
            h.now(),
            100,
            "100 ns DesSleep must resolve at sim_time == 100 exactly"
        );
    });
    let elapsed = start.elapsed();

    assert_eq!(handle.now(), 100, "simulator clock is at 100 ns after run");

    // We advanced 100 ns of simulator time. Even on slow CI runners,
    // a microsecond's worth of work shouldn't take half a second of
    // wall clock — anything close to that means we're accidentally
    // sleeping wall-clock somewhere.
    assert!(
        elapsed.as_millis() < 500,
        "wall-clock took {elapsed:?}; expected effectively zero"
    );
}

#[test]
fn back_to_back_sleeps_compose() {
    let sim = Simulator::new();
    sim.run(|h| async move {
        h.sleep_for(50).await;
        assert_eq!(h.now(), 50);
        h.sleep_for(25).await;
        assert_eq!(h.now(), 75);
        h.sleep_for(425).await;
        assert_eq!(h.now(), 500);
    });
}

#[test]
fn sleep_until_absolute_deadline_resolves_at_that_time() {
    let sim = Simulator::new();
    sim.run(|h| async move {
        h.sleep_until(1234).await;
        assert_eq!(h.now(), 1234);
        // Re-sleeping until a past deadline is a no-op (resolves
        // immediately, advances no time).
        h.sleep_until(500).await;
        assert_eq!(h.now(), 1234);
    });
}
