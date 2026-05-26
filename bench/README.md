# `bench/` — criterion microbenchmark history

This directory holds the long-running, append-only record of
microbenchmark numbers shipped by `cargo bench`. The format is a
single CSV file (`history.csv`) so the diff stays human-readable
and `git log -p bench/history.csv` is a literal performance
timeline.

## Schema (`history.csv`)

| column                  | type    | meaning                                                  |
| ----------------------- | ------- | -------------------------------------------------------- |
| `ts`                    | ISO 8601 | when the row was recorded (date precision sufficient)   |
| `bench_id`              | string  | criterion benchmark id, e.g. `b1::event_queue::push_then_pop/1m` |
| `median_ns`             | u64     | criterion-reported median wall time per iteration       |
| `mean_ns`               | u64     | criterion-reported mean wall time per iteration         |
| `low_ns`                | u64     | low end of criterion's 95 % confidence interval         |
| `high_ns`               | u64     | high end of criterion's 95 % confidence interval        |
| `throughput_elem_per_s` | u64     | events / second (criterion `Throughput::Elements`)      |
| `sample_size`           | u32     | criterion samples                                       |
| `host`                  | string  | machine descriptor (`arch-os-build-host`)               |
| `commit_short`          | string  | short git sha (`baseline` for the seed row)             |
| `notes`                 | string  | free text — context (`T1.12 baseline`, regression note) |

## Workflow

1. Run `cargo bench -p qwksim-core --bench event_queue` (or any
   bench under `crates/qwksim-*/benches/`).
2. Copy the numbers reported on the `Analyzing` line of each
   benchmark into a new row in `history.csv`.
3. Commit the change as `bench(<crate>): record <bench_id> on
   <host>` so future PRs can find it via `git log -p bench/`.

A nightly automation that performs steps 2–3 lands in T7.x; until
then the workflow is by-hand.

## What about criterion's own history?

Criterion writes per-run JSON to `target/criterion/`. That tree is
**not** checked in (`.gitignore`d via `/target`) and is overwritten
on every `cargo bench`. `history.csv` is the durable record.
