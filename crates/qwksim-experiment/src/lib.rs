//! Experiment harness for `qwksim`.
//!
//! Loads hierarchical TOML manifests (`[scenario]` + `[sweep]`,
//! Q15.1 = em2), expands sweeps into a `Vec<ResolvedScenario>`,
//! drives replicate execution via `rayon::par_iter` (capped at
//! `num_cpus - 1` to keep headroom for the OS), and writes per-run
//! Parquet files with the full provenance fingerprint embedded as
//! `key_value_metadata` (Q15.2 = pf3): git commit, `Cargo.lock`
//! hash, post-expansion config, seed, simulator version, vendor
//! calibration data hash, host CPU/RAM. Also writes the per-
//! experiment `index.parquet` (Q15.6 = sl2).
//!
//! Seeding discipline: each replicate derives its per-stream RNGs
//! from a `ReplicateSeed` via the hierarchical-split scheme in the
//! `qwksim-core` crate, enabling common-random-numbers (CRN) paired
//! comparisons across mechanisms.
//!
//! See `plan/solution_plan.md` §2.8.
