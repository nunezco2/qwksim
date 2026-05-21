//! **b5** — full single-run end-to-end benchmark.
//!
//! Measures one headline-shaped scenario (1,000 workflows, 1 simulated
//! hour, full granularity) wall-clock, gating the Q14.1 ≤ 60 s/run
//! ceiling. Stub for now; the real body wires up qwksim-experiment
//! and the headline scenario in T6.x.

use criterion::{criterion_group, criterion_main, Criterion};

fn b5_end_to_end_stub(c: &mut Criterion) {
    c.bench_function("b5::end_to_end::no_op_stub", |b| b.iter(|| ()));
}

criterion_group!(benches, b5_end_to_end_stub);
criterion_main!(benches);
