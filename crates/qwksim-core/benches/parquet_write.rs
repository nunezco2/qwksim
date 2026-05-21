//! **b4** — Parquet write throughput microbenchmark.
//!
//! Measures the cost of the `TelemetrySink` trait's Parquet writer
//! path (workflow + iteration + per-bargaining-round records),
//! ensuring telemetry never becomes a bottleneck of the inner DES
//! loop. Stub for now; the real body lands when the Parquet sink
//! arrives in T1.x.

use criterion::{criterion_group, criterion_main, Criterion};

fn b4_parquet_write_stub(c: &mut Criterion) {
    c.bench_function("b4::parquet_write::no_op_stub", |b| b.iter(|| ()));
}

criterion_group!(benches, b4_parquet_write_stub);
criterion_main!(benches);
