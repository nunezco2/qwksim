//! Per-replicate DES kernel — the `(M-A) pure-DES + tokio-ergonomics`
//! architecture (FLAG-M closed).
//!
//! Three public types:
//!
//! - [`Simulator`] owns the replicate-local state (simulator clock,
//!   event-sequence allocator, the pending-waker registry) and a
//!   private `tokio::runtime::Runtime` configured as
//!   `current_thread`. A single replicate spans a single thread; the
//!   outer `rayon::par_iter` (Q15.1, T6.x) drives replicates in
//!   parallel.
//! - [`SimulatorHandle`] is the cloneable handle agents use to read
//!   the simulator clock, allocate `EventSeq`s, and register
//!   logical-time waits.
//! - [`DesSleep`] is the **only** async primitive the simulator
//!   ships. It plays the role `tokio::time::sleep` would in a
//!   wall-clock runtime, but resolves against [`SimTime`] only —
//!   never against wall-clock. Hence no `enable_time` on the tokio
//!   runtime: simulator time is the DES logical clock, advanced
//!   exclusively by the driver loop in [`Simulator::run`].
//!
//! See `plan/solution_plan.md` §3.1 / §3.5 for the architecture.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use tokio::runtime::Builder;
use tokio::task::LocalSet;

use crate::event::{AgentId, EventSeq, SimTime};
use crate::queue::{EventQueue, EventSeqAllocator};

/// Sentinel `AgentId` used by [`DesSleep`] for sleeps that are not
/// owned by a specific agent (e.g. when an agent has not been
/// assigned an id yet). Never overlaps with a real `AgentId` because
/// real ids land in the lower half of the `u32` space by convention.
pub const AGENT_UNATTRIBUTED: AgentId = u32::MAX;

/// Replicate-local mutable state shared by [`Simulator`],
/// [`SimulatorHandle`], and every live [`DesSleep`] future.
///
/// `Rc<RefCell<…>>` (not `Arc<Mutex<…>>`): the entire replicate runs
/// on a single thread under `current_thread`, so atomic primitives
/// are pure overhead. Re-entrant borrows must be avoided in the
/// driver loop — see [`Simulator::run`] for the discipline.
struct SimState {
    /// Current simulator clock — the logical time the next event
    /// will fire at.
    now: SimTime,
    /// Reserved hook for the soon-to-arrive event-dispatch path
    /// (T1.4+). Kept here so future PRs do not have to thread an
    /// additional `Rc<RefCell<…>>` through the API.
    #[allow(dead_code)]
    queue: EventQueue,
    /// Monotonic `EventSeq` counter — the third coordinate of the
    /// `(at, agent, seq)` tie-break key. Issued by `DesSleep::poll`
    /// when it registers a waker.
    seq_alloc: EventSeqAllocator,
    /// Registered wakers, sorted ascending by their `(deadline,
    /// agent, seq)` triple. `pop_first` yields the earliest pending
    /// wake; equal deadlines break by `agent` then `seq` for total
    /// determinism (Q6′ = R2).
    pending_wakes: BTreeMap<(SimTime, AgentId, EventSeq), Waker>,
}

/// Cloneable handle that agents use to interact with the simulator
/// (`Rc` clones share the same underlying [`SimState`]).
#[derive(Clone)]
pub struct SimulatorHandle {
    state: Rc<RefCell<SimState>>,
}

impl SimulatorHandle {
    /// The current simulator clock.
    pub fn now(&self) -> SimTime {
        self.state.borrow().now
    }

    /// Allocate the next `EventSeq` for this replicate.
    pub fn next_seq(&self) -> EventSeq {
        self.state.borrow_mut().seq_alloc.allocate()
    }

    /// Return a future that resolves at `now() + dt` simulator-time
    /// nanoseconds from the moment of the call.
    pub fn sleep_for(&self, dt: SimTime) -> DesSleep {
        let deadline = self.state.borrow().now.saturating_add(dt);
        self.sleep_until(deadline)
    }

    /// Return a future that resolves at or after the absolute
    /// simulator-time `deadline`.
    pub fn sleep_until(&self, deadline: SimTime) -> DesSleep {
        DesSleep {
            state: self.state.clone(),
            deadline,
            registered_key: None,
            agent: AGENT_UNATTRIBUTED,
        }
    }

    /// Same as [`Self::sleep_until`] but attributes the wake to a
    /// specific `agent` for the tie-break key (lets two agents
    /// waiting on the same deadline have a deterministic relative
    /// order).
    pub fn sleep_until_as(&self, deadline: SimTime, agent: AgentId) -> DesSleep {
        DesSleep {
            state: self.state.clone(),
            deadline,
            registered_key: None,
            agent,
        }
    }
}

/// The DES-aware sleep future used in place of `tokio::time::sleep`.
///
/// On `poll`:
///
/// 1. If the future had previously registered a wake, drop the
///    stale entry. Re-entry happens when `poll` is invoked after a
///    spurious wake.
/// 2. If `now() >= deadline`, resolve immediately with `()`.
/// 3. Otherwise register the current task's waker keyed by
///    `(deadline, agent, seq)` (where `seq` is freshly allocated)
///    and return `Pending`.
///
/// Dropping a `DesSleep` does **not** automatically deregister an
/// already-registered waker — the driver loop tolerates stale
/// entries because the dropped task will not be polled again. A
/// future PR can add an explicit `Drop` impl if profiling shows the
/// pending-wake map growing without bound.
pub struct DesSleep {
    state: Rc<RefCell<SimState>>,
    deadline: SimTime,
    registered_key: Option<(SimTime, AgentId, EventSeq)>,
    agent: AgentId,
}

impl Future for DesSleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        // Take everything we need out of `self` before borrowing
        // through `self.state` — keeps the disjoint-field borrows
        // happy under `Pin<&mut Self>`.
        let old_key = self.registered_key.take();
        let deadline = self.deadline;
        let agent = self.agent;

        let mut state = self.state.borrow_mut();

        if let Some(old) = old_key {
            state.pending_wakes.remove(&old);
        }

        if state.now >= deadline {
            return Poll::Ready(());
        }

        let seq = state.seq_alloc.allocate();
        let key = (deadline, agent, seq);
        state.pending_wakes.insert(key, cx.waker().clone());
        drop(state); // release before mutating `self.registered_key`

        self.registered_key = Some(key);
        Poll::Pending
    }
}

/// Per-replicate DES simulator.
///
/// Owns the [`SimState`] and a private `current_thread` tokio
/// runtime. Cheap to construct (a few RefCell + BTreeMap
/// allocations); single-use — there is no `reset()`.
pub struct Simulator {
    state: Rc<RefCell<SimState>>,
    runtime: tokio::runtime::Runtime,
}

impl Simulator {
    /// Build a fresh simulator at `now = 0`.
    ///
    /// Panics if the tokio runtime cannot be constructed (an OS
    /// resource exhaustion that callers cannot meaningfully recover
    /// from in the per-replicate loop).
    pub fn new() -> Self {
        let runtime = Builder::new_current_thread()
            .build()
            .expect("build tokio current_thread runtime");
        Self {
            state: Rc::new(RefCell::new(SimState {
                now: 0,
                queue: EventQueue::new(),
                seq_alloc: EventSeqAllocator::new(),
                pending_wakes: BTreeMap::new(),
            })),
            runtime,
        }
    }

    /// A fresh [`SimulatorHandle`] that shares this simulator's
    /// state.
    pub fn handle(&self) -> SimulatorHandle {
        SimulatorHandle {
            state: self.state.clone(),
        }
    }

    /// Drive the simulation: spawn the agent-set future
    /// `setup(handle)` on a fresh [`LocalSet`], then loop
    /// advancing simulator time to the next registered wake and
    /// firing the corresponding wakers, until no waker is
    /// registered.
    ///
    /// `setup` is called once and is responsible for `spawn_local`
    /// of any further agents.
    pub fn run<F, Fut>(&self, setup: F)
    where
        F: FnOnce(SimulatorHandle) -> Fut,
        Fut: Future<Output = ()> + 'static,
    {
        let state = self.state.clone();
        let handle = SimulatorHandle {
            state: state.clone(),
        };
        let local = LocalSet::new();

        self.runtime.block_on(local.run_until(async move {
            tokio::task::spawn_local(setup(handle));

            loop {
                // Yield control so agents get polled to their next
                // .await point. After this returns, every spawned
                // task is either complete or registered a waker.
                tokio::task::yield_now().await;

                let next_deadline = {
                    let s = state.borrow();
                    s.pending_wakes.keys().next().map(|k| k.0)
                };

                let Some(deadline) = next_deadline else {
                    break;
                };

                state.borrow_mut().now = deadline;

                // Wake every entry due at this deadline. Pop one at
                // a time to keep the borrow region small.
                loop {
                    let woken = {
                        let mut s = state.borrow_mut();
                        match s.pending_wakes.keys().next().copied() {
                            Some(k) if k.0 <= deadline => s.pending_wakes.remove(&k),
                            _ => None,
                        }
                    };
                    match woken {
                        Some(waker) => waker.wake(),
                        None => break,
                    }
                }
            }
        }));
    }
}

impl Default for Simulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn handle_starts_at_zero() {
        let sim = Simulator::new();
        assert_eq!(sim.handle().now(), 0);
    }

    #[test]
    fn sleep_resolves_immediately_when_deadline_already_past() {
        let sim = Simulator::new();
        let h = sim.handle();
        sim.run(move |handle| async move {
            // `now() == 0`, so a sleep_until(0) must resolve
            // without ever registering a waker.
            handle.sleep_until(0).await;
            assert_eq!(handle.now(), 0);
        });
        let _ = h;
    }

    #[test]
    fn run_drives_an_empty_agent_set_to_completion() {
        let sim = Simulator::new();
        sim.run(|_| async {});
        assert_eq!(sim.handle().now(), 0);
    }

    #[test]
    fn sleep_advances_simulator_time_with_no_wall_clock_delay() {
        let sim = Simulator::new();
        let h = sim.handle();

        let start = Instant::now();
        sim.run(move |handle| async move {
            assert_eq!(handle.now(), 0);
            handle.sleep_for(100).await;
            assert_eq!(handle.now(), 100);
            handle.sleep_for(900).await;
            assert_eq!(handle.now(), 1000);
        });
        let elapsed = start.elapsed();

        assert_eq!(h.now(), 1000);
        // Generous bound — only meant to prove we did not wait
        // wall-clock for 1000 ns × wall-clock factor.
        assert!(
            elapsed.as_millis() < 500,
            "simulator advanced 1000 ns but took {elapsed:?} of wall clock"
        );
    }
}
