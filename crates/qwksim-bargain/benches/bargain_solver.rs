//! **b2** — Nash-product evaluator + best-response round benchmark.
//!
//! Measures one full bargaining round at the headline super-site
//! population (`n = 3 + k` agents, with `k` QPUs). The inner loop
//! dominates per-bargain wall-clock and gates the Q14.1 budget.
//! Stub for now; the real body lands when the solver arrives in
//! T4.x.

use criterion::{criterion_group, criterion_main, Criterion};

fn b2_bargain_solver_stub(c: &mut Criterion) {
    c.bench_function("b2::bargain_solver::no_op_stub", |b| b.iter(|| ()));
}

criterion_group!(benches, b2_bargain_solver_stub);
criterion_main!(benches);
