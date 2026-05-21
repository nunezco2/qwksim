//! Command-line interface for `qwksim`.
//!
//! Single binary (`qwksim`) tying every member crate together. The
//! `clap`-driven sub-command surface (Q15.4 = rt3) is:
//!
//! - `qwksim run <scenario>` — execute a single scenario, one replicate.
//! - `qwksim sweep <manifest>` — expand a sweep manifest and run the
//!   full replicate matrix under `rayon::par_iter`.
//! - `qwksim replay <run.parquet>` — re-run the exact scenario from
//!   embedded provenance and assert bit-identical output (CI gate).
//! - `qwksim perturb <run.parquet> --agent <id> --field <name> --delta <v>`
//!   — counterfactual run for the empirical strategy-proofness test
//!   (Q2.3 + Q6′ = R2).
//! - `qwksim diff <a.parquet> <b.parquet>` — column-wise diff.
//! - `qwksim report <experiment_name>` — drive the figure factory
//!   in the `qwksim-analysis` crate and emit the artefact folder.
//!
//! Also installs the `#[global_allocator] = mimalloc` (Q14.4 = a2)
//! and wires the `tracing` + JSON + `EnvFilter` stack (Q13.4 = tl1).
//!
//! See `plan/solution_plan.md` §2.10.

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() {
    // Sub-command dispatch wired in T6.x (CLI scaffolding lands then).
}
