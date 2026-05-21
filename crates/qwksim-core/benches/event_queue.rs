//! **b1** — event-queue insert/pop microbenchmark.
//!
//! Tracks the cost of the simulator's `BinaryHeap<Event>` event queue
//! under the deterministic `(at, agent, seq)` tie-break key. Stub for
//! now; the real body lands together with the event-queue
//! implementation in T1.4 (queue) and T1.5 (deterministic tie-break).

use criterion::{criterion_group, criterion_main, Criterion};

fn b1_event_queue_stub(c: &mut Criterion) {
    c.bench_function("b1::event_queue::no_op_stub", |b| b.iter(|| ()));
}

criterion_group!(benches, b1_event_queue_stub);
criterion_main!(benches);
