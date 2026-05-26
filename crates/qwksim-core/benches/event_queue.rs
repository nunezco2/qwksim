//! **b1** — event-queue insert/pop microbenchmark.
//!
//! Tracks the cost of the simulator's `BinaryHeap<Event>` event
//! queue (T1.2) under the deterministic `(at, agent, seq)` tie-break
//! key (T1.1). Two cases:
//!
//! - **`push_then_pop_1m`** — bulk-load 1 000 000 events with
//!   semi-random `at` and `agent`, then drain the queue. Exercises
//!   the heapify path with a deep queue.
//! - **`steady_state_1m_mix`** — prime the queue to a target depth
//!   of ~10 000 in-flight events, then run 1 000 000 pop+push
//!   cycles where each popped event schedules a successor.
//!   Mimics the inner-loop shape of the DES kernel (T1.3 +
//!   future T2.x/T4.x callers).
//!
//! Both cases use `Throughput::Elements`, so criterion reports
//! throughput in events/second alongside the per-iteration mean.

use criterion::{criterion_group, criterion_main, Criterion, Throughput};

use qwksim_core::event::{AgentId, Event, EventKind, SimTime};
use qwksim_core::queue::{EventQueue, EventSeqAllocator};

const EVENTS: u64 = 1_000_000;
const STEADY_PRIME: u64 = 10_000;

fn synthetic_event(i: u64, alloc: &mut EventSeqAllocator) -> Event {
    Event {
        // 17 and 31 are coprime to the agent ring size; gives a
        // long-period semi-random shape without the cost of a
        // real PRNG inside the bench loop.
        at: ((i.wrapping_mul(17).wrapping_add(31)) as SimTime),
        agent: ((i % 13) as AgentId),
        seq: alloc.allocate(),
        kind: EventKind::Arrival,
    }
}

fn bench_push_then_pop_1m(c: &mut Criterion) {
    let mut group = c.benchmark_group("b1::event_queue::push_then_pop");
    group.throughput(Throughput::Elements(EVENTS));
    group.bench_function("1m", |b| {
        b.iter(|| {
            let mut queue = EventQueue::with_capacity(EVENTS as usize);
            let mut alloc = EventSeqAllocator::new();
            for i in 0..EVENTS {
                queue.push(synthetic_event(i, &mut alloc));
            }
            while queue.pop().is_some() {}
        });
    });
    group.finish();
}

fn bench_steady_state_1m_mix(c: &mut Criterion) {
    let mut group = c.benchmark_group("b1::event_queue::steady_state");
    group.throughput(Throughput::Elements(EVENTS));
    group.bench_function("1m_mix", |b| {
        b.iter_with_setup(
            || {
                // Setup runs outside the timed region: prime the
                // queue to STEADY_PRIME in-flight events.
                let mut queue = EventQueue::with_capacity(STEADY_PRIME as usize + 1024);
                let mut alloc = EventSeqAllocator::new();
                for i in 0..STEADY_PRIME {
                    queue.push(synthetic_event(i, &mut alloc));
                }
                (queue, alloc)
            },
            |(mut queue, mut alloc)| {
                // EVENTS pop+push cycles. Each popped event
                // schedules a successor ~17 ns later (the
                // arithmetic shape of synthetic_event).
                for i in STEADY_PRIME..STEADY_PRIME + EVENTS {
                    let popped = queue.pop().expect("queue primed");
                    let mut next = synthetic_event(i, &mut alloc);
                    next.at = popped.at.wrapping_add(17);
                    queue.push(next);
                }
            },
        );
    });
    group.finish();
}

criterion_group!(benches, bench_push_then_pop_1m, bench_steady_state_1m_mix);
criterion_main!(benches);
