//! `Fcfs` — first-come-first-served scheduler against a single
//! [`HpcPartitionAgent`].
//!
//! The strict no-knowledge control baseline in the 2×2 factorial.
//! Workflows are admitted in arrival order; when the partition is
//! full, queued workflows wait for enough in-flight jobs to
//! complete to free their requested capacity, then admit in the
//! same order they queued. Service time is treated as deterministic
//! (real jitter lands when [`qwksim_workflow`] provides the
//! `TaskDuration` stream in Phase 2).
//!
//! The scheduler is **closed-form**: every call to
//! [`Fcfs::submit`] returns the *exact* simulator-time completion
//! the workflow will see. The implementation maintains a
//! `BTreeMap<completion_time, cores>` of in-flight reservations
//! and a `VecDeque<PendingArrival>` of queued workflows.

use std::collections::{BTreeMap, VecDeque};

use qwksim_core::event::{Event, SimTime};
use qwksim_resources::{Allocation, HpcPartitionAgent, ResourceAgent};
use qwksim_scheduler::{Scheduler, SchedulingDecision, View, Workflow};

#[derive(Debug)]
struct PendingArrival {
    arrival: SimTime,
    cores: u32,
    service_ns: SimTime,
}

/// First-come-first-served scheduler over an `HpcPartitionAgent`.
///
/// Internal-only state — methods take `&mut self`. Driven by the
/// domain-specific [`Fcfs::submit`] method today; the
/// [`Scheduler`] trait impls are no-ops until the Phase-2 workflow
/// runtime carries enough information through the trait surface to
/// power them.
#[derive(Debug)]
pub struct Fcfs {
    partition: HpcPartitionAgent,
    /// `completion_time → cores held by that completion`. Sorted by
    /// completion time so the earliest finish is `iter().next()`.
    in_flight: BTreeMap<SimTime, u32>,
    queue: VecDeque<PendingArrival>,
}

impl Fcfs {
    /// Build a scheduler over the partition agent `partition`. The
    /// agent must not yet have any allocations (the scheduler
    /// expects a clean slate to track ownership of every release).
    pub fn new(partition: HpcPartitionAgent) -> Self {
        assert_eq!(
            partition.used_cores(),
            0,
            "Fcfs::new requires an HpcPartitionAgent at full free capacity"
        );
        Self {
            partition,
            in_flight: BTreeMap::new(),
            queue: VecDeque::new(),
        }
    }

    /// Borrow the underlying partition agent (read-only).
    pub fn partition(&self) -> &HpcPartitionAgent {
        &self.partition
    }

    /// Submit a single workflow arrival at simulator-time
    /// `arrival_ns` requesting `cores` cores for a deterministic
    /// `service_ns` simulator-nanoseconds. Returns the completion
    /// time the workflow will see — admission may be delayed if
    /// the partition is full at `arrival_ns`, in which case the
    /// returned completion is later than `arrival_ns + service_ns`.
    ///
    /// # Panics
    /// Panics if `cores` exceeds the partition's total capacity —
    /// the FCFS queue would never drain.
    pub fn submit(&mut self, arrival_ns: SimTime, cores: u32, service_ns: SimTime) -> SimTime {
        assert!(
            cores > 0 && cores <= self.partition.total_cores(),
            "Fcfs::submit: cores {cores} not in (0, {total}]",
            total = self.partition.total_cores()
        );
        // First drop any in-flight reservations that have already
        // completed by `arrival_ns`.
        self.advance_to(arrival_ns);

        // Try to start anything queued ahead of this arrival; if
        // the queue drains, this arrival can compete for the same
        // start instant.
        self.drain_queue(arrival_ns);

        // If the queue is non-empty, FIFO dictates this arrival
        // waits behind it. Otherwise, see if we can start now.
        if self.queue.is_empty() && self.partition.true_free_cores() >= cores {
            return self.start(arrival_ns, cores, service_ns);
        }

        // Queue and compute the deterministic completion time by
        // replaying the in-flight calendar (no actual state
        // mutation beyond what the start() calls above did).
        self.queue.push_back(PendingArrival {
            arrival: arrival_ns,
            cores,
            service_ns,
        });
        self.flush_queue_until_admit(arrival_ns)
    }

    fn start(&mut self, start: SimTime, cores: u32, service_ns: SimTime) -> SimTime {
        self.partition.accept(
            Allocation {
                cores,
                ..Default::default()
            },
            start,
        );
        let completion = start.saturating_add(service_ns);
        *self.in_flight.entry(completion).or_insert(0) += cores;
        completion
    }

    fn advance_to(&mut self, now: SimTime) {
        let completed: Vec<SimTime> = self.in_flight.range(..=now).map(|(&t, _)| t).collect();
        for t in completed {
            let cores = self.in_flight.remove(&t).unwrap();
            self.partition.release(
                &Allocation {
                    cores,
                    ..Default::default()
                },
                t,
            );
        }
    }

    /// Pop completions one at a time, advancing `now` to each
    /// completion's instant, until the head of `self.queue` can
    /// start or the queue is empty. Pulled out so both the "drain
    /// before admitting" and "flush until *this* arrival admits"
    /// loops share a single implementation.
    fn drain_queue(&mut self, mut now: SimTime) {
        while let Some(head) = self.queue.front() {
            let needed = head.cores;
            if self.partition.true_free_cores() >= needed {
                let head = self.queue.pop_front().unwrap();
                let start = now.max(head.arrival);
                self.start(start, head.cores, head.service_ns);
                continue;
            }
            // Need more capacity — advance to the next in-flight
            // completion, release it, and retry.
            match self.in_flight.iter().next() {
                Some((&next_t, _)) => {
                    now = now.max(next_t);
                    self.advance_to(now);
                }
                None => {
                    // No in-flight reservations and not enough
                    // free → caller is over-committing relative
                    // to partition.total_cores(); the submit guard
                    // catches this for individual arrivals.
                    break;
                }
            }
        }
    }

    /// As above, but specifically: drive the queue until the last
    /// element (the one just enqueued by `submit`) starts, and
    /// return its completion time.
    fn flush_queue_until_admit(&mut self, arrival: SimTime) -> SimTime {
        let arrival_target = self.queue.back().expect("just pushed").arrival;
        debug_assert_eq!(arrival_target, arrival);
        let target_len = self.queue.len();

        let mut now = arrival;
        while self.queue.len() == target_len {
            // No new starts; we are still queued. Release the
            // earliest completion to free capacity.
            let &next_t = self
                .in_flight
                .iter()
                .next()
                .map(|(t, _)| t)
                .expect("queued workflow but nothing in flight — impossible by submit guard");
            now = now.max(next_t);
            self.advance_to(now);
            self.drain_queue(now);
        }
        // Last `in_flight` insertion corresponds to the workflow
        // we just admitted (drain_queue starts the head of the
        // queue using `start()`, which inserts into `in_flight`).
        // Look up its completion: it is the *latest* completion
        // currently in the calendar that was inserted at `now`.
        *self
            .in_flight
            .iter()
            .next_back()
            .map(|(t, _)| t)
            .expect("just started a workflow but in_flight is empty")
    }
}

impl Scheduler for Fcfs {
    fn on_arrival(&mut self, _workflow: &Workflow, _view: View<'_>) -> SchedulingDecision {
        // Placeholder: the Workflow marker type does not yet carry
        // (arrival, cores, service_ns). Real wiring lands once
        // qwksim-workflow provides the concrete Workflow schema in
        // Phase 2; today the closed-form `Fcfs::submit` is the
        // exercise-the-logic API.
        SchedulingDecision
    }

    fn on_event(&mut self, _event: &Event, _view: View<'_>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh(total_cores: u32) -> Fcfs {
        Fcfs::new(HpcPartitionAgent::new(1, total_cores))
    }

    #[test]
    fn three_spaced_arrivals_complete_at_arrival_plus_service() {
        let mut f = fresh(1);
        assert_eq!(f.submit(0, 1, 5), 5);
        assert_eq!(f.submit(10, 1, 5), 15);
        assert_eq!(f.submit(20, 1, 5), 25);
    }

    #[test]
    fn arrivals_serialise_when_partition_is_too_small_for_concurrency() {
        // One-core partition, all three jobs arrive at time 0.
        let mut f = fresh(1);
        assert_eq!(f.submit(0, 1, 5), 5);
        assert_eq!(f.submit(0, 1, 5), 10);
        assert_eq!(f.submit(0, 1, 5), 15);
        assert_eq!(f.partition().used_cores(), 1);
    }

    #[test]
    fn two_core_partition_runs_two_jobs_concurrently() {
        let mut f = fresh(2);
        assert_eq!(f.submit(0, 1, 5), 5);
        assert_eq!(f.submit(0, 1, 5), 5);
        assert_eq!(f.submit(0, 1, 5), 10);
    }
}
