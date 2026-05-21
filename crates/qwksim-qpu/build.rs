//! Build script for `qwksim-qpu`: materialise per-modality calibration
//! files in `$OUT_DIR`.
//!
//! For each `[[file]]` entry in `vendor.toml`, the script chooses
//! between two sources:
//!
//!   - **Vendor.** If `<workspace_root>/data/vendor/<name>` exists and
//!     its SHA-256 matches the entry's `sha256` field, the vendor
//!     file is copied to `$OUT_DIR/<name>`.
//!   - **Synthetic fallback.** Otherwise, `synthetic/<name>` (shipped
//!     in this crate) is copied to `$OUT_DIR/<name>`. The synthetic
//!     placeholder is deterministic — its bytes are fixed by what is
//!     checked in — so builds with no network and no vendor data
//!     always succeed.
//!
//! Both branches write the same path in `$OUT_DIR`, so downstream
//! source can `include_bytes!(concat!(env!("OUT_DIR"),
//! "/calibration_<modality>.json"))` without caring which source
//! produced it.
//!
//! Per Q17.2 = (vc3): vendor-data licence audit is pending, so every
//! `sha256` entry in `vendor.toml` is the sentinel
//! `PLACEHOLDER_NOT_YET_AUDITED` today. The vendor branch is dead
//! code in this PR but exercised once a real SHA is checked in. The
//! synthetic branch is the live one.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const VENDOR_TOML: &str = "vendor.toml";
const VENDOR_DIR_FROM_CRATE: &str = "../../data/vendor";
const SYNTHETIC_DIR_FROM_CRATE: &str = "synthetic";

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    let vendor_manifest = manifest_dir.join(VENDOR_TOML);
    println!("cargo:rerun-if-changed={}", vendor_manifest.display());

    let manifest_text = fs::read_to_string(&vendor_manifest).expect("read vendor.toml");
    let parsed: toml::Value = toml::from_str(&manifest_text).expect("parse vendor.toml");
    let entries = parsed
        .get("file")
        .and_then(|f| f.as_array())
        .expect("vendor.toml must contain [[file]] entries");

    for entry in entries {
        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .expect("file.name string");
        let expected_sha = entry
            .get("sha256")
            .and_then(|v| v.as_str())
            .expect("file.sha256 string");

        let vendor_path = manifest_dir.join(VENDOR_DIR_FROM_CRATE).join(name);
        let synthetic_path = manifest_dir.join(SYNTHETIC_DIR_FROM_CRATE).join(name);
        let out_path = out_dir.join(name);

        println!("cargo:rerun-if-changed={}", vendor_path.display());
        println!("cargo:rerun-if-changed={}", synthetic_path.display());

        if vendor_path.exists() && hash_matches(&vendor_path, expected_sha) {
            fs::copy(&vendor_path, &out_path).expect("copy vendor → OUT_DIR");
        } else {
            fs::copy(&synthetic_path, &out_path).expect("copy synthetic → OUT_DIR");
        }
    }
}

fn hash_matches(path: &Path, expected_hex: &str) -> bool {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual = hex_lower(hasher.finalize().as_slice());
    actual.eq_ignore_ascii_case(expected_hex)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}
