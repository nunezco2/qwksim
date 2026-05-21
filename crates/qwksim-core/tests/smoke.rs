//! Smoke integration test for `qwksim-core`.
//!
//! Asserts the crate compiles, links, and exposes its sentinel
//! `VERSION` constant matching the `Cargo.toml` package version.
//! Establishes the integration-test harness for later, more
//! substantive tests.

#[test]
fn version_matches_cargo_pkg_version() {
    assert_eq!(qwksim_core::VERSION, env!("CARGO_PKG_VERSION"));
}
