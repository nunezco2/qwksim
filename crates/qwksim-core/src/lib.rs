//! Discrete-event simulator kernel for `qwksim`.
//!
//! Owns the simulator clock and event types (`SimTime`, `AgentId`,
//! `EventSeq`, `Event`, `EventKind`), the deterministic event queue
//! with `(at, agent, seq)` tie-breaking, the hierarchical RNG
//! (master → replicate → stream) built on `rand_chacha::ChaCha20Rng`,
//! logical-time channels for the `(M-A)` pure-DES + tokio-ergonomics
//! architecture, and the `TelemetrySink` trait through which all
//! observable simulation state is written to the Parquet output layer.
//!
//! See `plan/solution_plan.md` §3 for the architectural specification.

/// Sentinel version string mirroring this crate's `Cargo.toml`.
///
/// Used by the integration smoke test to assert that the crate
/// compiles and links correctly. Real callers should prefer
/// `env!("CARGO_PKG_VERSION")` at their own call site.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
