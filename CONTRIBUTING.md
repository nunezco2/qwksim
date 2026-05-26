# Contributing to qwksim

Thanks for your interest in contributing. This project is small (2–5
contributors); we keep process light and reviewer-facing artefacts
clean.

- [Code of Conduct](#code-of-conduct)
- [Issue and PR workflow](#issue-and-pr-workflow)
- [Developer Certificate of Origin (DCO)](#developer-certificate-of-origin-dco)
- [Style and conventions](#style-and-conventions)
- [License of contributions](#license-of-contributions)

## Code of Conduct

This project adopts the [Contributor Covenant 2.1](CODE_OF_CONDUCT.md).
By participating you agree to abide by it.

## Issue and PR workflow

The project is organised as a sequence of small, dependency-ordered
tasks. Each task lives as a GitHub issue (`[T{phase}.{n}] …`) and is
delivered by one pull request.

1. Pick an issue whose `Depends on` list is satisfied (all referenced
   issues closed). If unsure, ask in the issue thread.
2. Branch from `main` (or from the dependency branch if it has not yet
   merged) with a name of the form `task/T{phase}.{n}-{short-slug}`.
3. Implement the deliverables listed in the issue's **Summary** and
   verify the **Acceptance criteria** locally.
4. Commit (see DCO sign-off below). PR title should match the issue's
   **Suggested PR title** field.
5. Open the PR against `main`. The body should include `Closes #N`
   so the issue auto-closes on merge.
6. CI must be green; address review comments by amending or adding
   commits on the branch.

## Developer Certificate of Origin (DCO)

We use the [Developer Certificate of Origin][dco] (DCO) version 1.1
in place of a Contributor License Agreement. The DCO is a lightweight
attestation: by signing off on your commits you certify that you have
the right to submit them under the project's license (Apache-2.0).

### How to sign off

Add a `Signed-off-by` trailer to every commit. The simplest way is the
`-s` (`--signoff`) flag:

```sh
git commit -s -m "feat(qwksim-core): add ChaCha20 stream split"
```

This appends a trailer of the form:

```
Signed-off-by: Your Name <you@example.com>
```

The name and email must match the values configured in
`user.name` / `user.email`. If they do not, run:

```sh
git config user.name  "Your Name"
git config user.email "you@example.com"
```

If you forgot `-s` on a single commit, amend it:

```sh
git commit --amend --signoff
```

For an entire branch:

```sh
git rebase --signoff main
```

### The DCO text

Below is the DCO 1.1 verbatim. Sign-off constitutes agreement.

> Developer Certificate of Origin
> Version 1.1
>
> Copyright (C) 2004, 2006 The Linux Foundation and its contributors.
>
> Everyone is permitted to copy and distribute verbatim copies of this
> license document, but changing it is not allowed.
>
> Developer's Certificate of Origin 1.1
>
> By making a contribution to this project, I certify that:
>
> (a) The contribution was created in whole or in part by me and I
>     have the right to submit it under the open source license
>     indicated in the file; or
>
> (b) The contribution is based upon previous work that, to the best
>     of my knowledge, is covered under an appropriate open source
>     license and I have the right under that license to submit that
>     work with modifications, whether created in whole or in part
>     by me, under the same open source license (unless I am
>     permitted to submit under a different license), as indicated
>     in the file; or
>
> (c) The contribution was provided directly to me by some other
>     person who certified (a), (b) or (c) and I have not modified
>     it.
>
> (d) I understand and agree that this project and the contribution
>     are public and that a record of the contribution (including all
>     personal information I submit with it, including my sign-off) is
>     maintained indefinitely and may be redistributed consistent with
>     this project or the open source license(s) involved.

[dco]: https://developercertificate.org/

## Style and conventions

- **Formatting:** `cargo fmt --all` before committing. CI runs
  `cargo fmt --check`.
- **Lints:** `cargo clippy --workspace --all-targets -- -D warnings`
  must pass.
- **Tests:** `cargo test --workspace`. Add unit tests next to the code
  they cover; integration tests under `tests/` of the owning crate.
- **Commit messages:** [Conventional Commits][cc]. Subject ≤ 72
  characters; imperative mood; scope is the crate name when
  applicable (e.g. `feat(qwksim-bargain): …`, `test(qwksim-core): …`,
  `docs(readme): …`, `ci: …`, `chore(repo): …`).
- **PR titles** mirror the conventional-commit format and match the
  issue's `Suggested PR title` field.
- **Determinism:** new code that touches simulator state must respect
  the per-replicate deterministic-replay guarantee. See
  `plan/solution_plan.md` §3. Concretely:
  - **No `HashMap` iteration on simulator-stateful paths.** Clippy
    is configured (via the workspace `[workspace.lints.clippy]`
    table + `clippy.toml`) to **deny** the `disallowed_methods`
    lint for `HashMap::iter`/`iter_mut`/`keys`/`values`/`values_mut`/
    `into_iter`/`into_keys`/`into_values`/`drain`. Use
    `BTreeMap` (key-ordered) on stateful paths; `HashMap` is fine
    for **point-lookup-only** state. The
    `crates/qwksim-experiment/tests/determinism_replay.rs` test
    is the runtime gate that catches any remaining
    non-determinism by byte-comparing three consecutive
    smoke-run Parquet outputs.
  - **Documented exceptions.** A handful of `HashMap` iterations
    will be unavoidable later (e.g. the `CompilationCache` in
    `qwksim-qpu` planned for T3.x, which uses `HashMap` for pure
    point-lookup with no iteration on the simulator-state path).
    These opt out per call site with
    `#[allow(clippy::disallowed_methods)] // <reason>` plus a
    one-line comment that names the exception in the docstring.

[cc]: https://www.conventionalcommits.org/en/v1.0.0/

## License of contributions

Contributions are made under the terms of the project's license
([Apache-2.0](LICENSE)). Your DCO sign-off attests that you have the
right to do so.
