# Vendor calibration data — licensing audit

**Status:** open · **Owner:** `<MAINTAINER>` · **Last reviewed:** _never_ ·
**Blocks:** the vendor branch of `qwksim-qpu/build.rs` (issue T0.8 / #8) and
the headline run.

## Background

Q17.2 of the project specification selected option **(vc3)**: defer the
vendor-data licensing decision to a one-shot audit; in the meantime the
build script falls back to the deterministic synthetic anchors under
`crates/qwksim-qpu/synthetic/` (the **(vd2)** path).

This document is the tracker for that audit. Each row records one
candidate public calibration data source and the per-source decision
between:

- **(vd1)** check the file directly into `data/vendor/` with attribution —
  preferred when redistribution is unambiguously permitted (e.g. CC-BY,
  Apache-2.0, explicit "you may redistribute" in vendor documentation).
- **(vd2)** keep the synthetic fallback as the headline source; *optionally*
  fetch the vendor file at build time with `build.rs` + SHA-256
  verification — required when redistribution is restricted but
  point-in-time use is permitted.
- **(vd3)** runtime fetch only (no redistribution, no build-time fetch).
  Rejected under FLAG-L (source-only release) per `plan/solution_plan.md`.

## Sources

| # | Modality | Source | Vendor URL | Snapshot date | Licence / redistribution terms | Decision (vd1 / vd2) | Notes |
|---|----------|--------|------------|---------------|--------------------------------|----------------------|-------|
| 1 | Superconducting | IBM Quantum public calibration sheets | <https://quantum.ibm.com> · per-device calibration JSON via IBM Quantum Platform | TODO | TODO — read IBM Quantum Terms of Use | TODO | Site_1 anchor (`calibration_superconducting.json`). Confirm the calibration JSON falls under either the IBM Quantum Terms or a separately published open-data licence. |
| 2 | Trapped ion   | IonQ public calibration data           | <https://ionq.com/quantum-systems> · published noise-model snapshots | TODO | TODO — read IonQ documentation / contact IonQ DevRel | TODO | Site_2 anchor (`calibration_trapped_ion.json`). Falls back to synthetic by default; promote to vd1 if redistribution is permitted, else stay vd2 with a build-time fetch. |

Add a new row whenever a new modality enters the headline (e.g. Site_3
photonic if `sw5` is enabled — see Q11.5).

## Decision template

For each source above, fill in:

```
- snapshot URL exactly as fetched: ___
- snapshot file SHA-256 (lowercase hex): ___
- redistribution clause cited (URL + verbatim quote): ___
- maintainer-of-record signing off: ___
- date of decision: YYYY-MM-DD
- chosen path: vd1 | vd2-with-build-fetch | vd2-synthetic-only
- if vd1: file path under `data/vendor/<name>.json` + line in `crates/qwksim-qpu/vendor.toml` updated to the real SHA-256.
- if vd2-with-build-fetch: build.rs URL + retry policy + the same SHA-256 line in `vendor.toml`.
- if vd2-synthetic-only: leave `vendor.toml` SHA at `PLACEHOLDER_NOT_YET_AUDITED`; document the rationale here.
```

## How this closes

This document closes when every row has an unambiguous decision recorded,
the corresponding entries in `crates/qwksim-qpu/vendor.toml` are updated,
and (if any vd1 row) the source files are committed under
`data/vendor/<name>.json`.

Until then, the headline runs use synthetic anchors and the simulator
remains honest about the source of every realised-fidelity number via
the `"source": "qwksim-synthetic-v1"` field in each calibration JSON.
