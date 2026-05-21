//! Analysis and figure factory for `qwksim`.
//!
//! Exposes the `Aggregator` API that scans per-run Parquet on the
//! fly (`polars::scan_parquet`), the statistical machinery used by
//! the headline summary table — paired Wilcoxon signed-rank test
//! under common random numbers (Q16.3 = t2), Cliff's δ effect size
//! (Q16.3 = t3), 4-D hypervolume calculator, and non-dominated-set
//! extraction for the Pareto frontier — and the `plotters`-driven
//! figure factory that emits the `qwksim report <experiment_name>`
//! artefact: a folder of SVG/PDF figures + CSV tables + `INDEX.md`
//! (Q16.5 = rp1).
//!
//! Figure inventory per experiment: 6 pairwise Pareto scatter
//! panels, 1 parallel-coordinates plot, 1 hypervolume bar (Q16.1 =
//! pf5); 4× per-metric per-mechanism + 4× difference heatmaps with
//! significance overlay (Q16.6 = ss3); per-scenario diagnostics
//! d1–d5 (Q16.2; d4 emitted only if per-bargaining-round telemetry
//! is present).
//!
//! See `plan/solution_plan.md` §2.9.
