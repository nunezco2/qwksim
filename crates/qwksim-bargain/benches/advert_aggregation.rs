//! **b3** — advertisement-broadcast aggregation benchmark.
//!
//! Measures the per-τ_adv-tick cost of every per-resource agent
//! aggregating its local state into the advertised summary the
//! bargaining mechanism consumes (free capacity per resource class,
//! expected wait, fidelity-class availability). Stub for now; the
//! real body lands when the advertisement protocol arrives in T4.x.

use criterion::{criterion_group, criterion_main, Criterion};

fn b3_advert_aggregation_stub(c: &mut Criterion) {
    c.bench_function("b3::advert_aggregation::no_op_stub", |b| b.iter(|| ()));
}

criterion_group!(benches, b3_advert_aggregation_stub);
criterion_main!(benches);
