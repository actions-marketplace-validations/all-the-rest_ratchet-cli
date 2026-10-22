# AGENTS.todo.md — open work

This file is a **log, not an order**: it records decisions and open work. Logging
is mandatory; working an entry off needs an explicit order. Verified-completed
entries are removed — the history lives in the commits.

The normative behaviour of the binary lives in `SPEC.md`; the rules for working in
this repo live in `AGENTS.md`. This file holds neither.

The **severity levels and the statuses** are not decided here: they are defined
once in the `ratchet-gates` skill
(`~/dev/agents-skills/.agents/skills/ratchet-gates/references/vocabulary.md`) and
the binary is specified to enforce exactly those values. A level or a status
outside that vocabulary is a hard error, never a fallback.

## Open

### The gates fail on a platform without symlinks

Logged 2026-10-21 (found by the verification of the path containment work).

**Why.** The CI runs on Linux and macOS only, so the run stays green there —
but the binary is released for Windows, and a contributor on Windows cannot
run the gate. Two defects, both older than the path work, were found while
simulating a non-Unix build (every `cfg(unix)` in `tests/` replaced by
`cfg(any())`):

1. `tests/update.rs` has a helper (`newer_release`) that is only used from
   `#[cfg(unix)]` code. On a platform without symlinks it is dead code, and
   `cargo clippy --all-targets -- -D warnings` fails on it.
2. Two tests in `tests/release.rs` (`a_symlinked_changelog_outside_the_root_is_refused`,
   `a_version_file_that_cannot_be_written_leaves_the_tree_alone`) put their
   `#[cfg(unix)]` block **inside** the test body: the setup is skipped while
   the assertions still run, so the tests fail. The condition belongs on the
   test function, not on a block inside it.
3. Two tests in `tests/cli.rs` (`a_symlinked_snapshot_is_refused`,
   `a_snapshot_that_leaves_the_root_through_a_link_is_refused`) call
   `std::os::unix::fs::symlink` **without** a `#[cfg(unix)]` guard — they do
   not compile on a platform without symlinks. The simulation cannot see this
   (it only rewrites the attributes), which is why it was missed.

A **fourth** failure in the simulation (`tests/action.rs`,
`the_validate_mode_runs_the_gates_without_a_reason` — "Permission denied") is
an artifact of the simulation and not a defect: the stub's executable bit is
set inside a `#[cfg(unix)]` block, and the simulation strips the block. On a
platform without symlinks there is no executable bit at all and the stub runs
anyway. The simulation's full result: cli 209 ✓, registry 26 ✓, report 25 ✓,
release 42 with 2 failed, action 35 with 1 failed (the artifact), update 14 ✓,
and the one clippy dead-code error of item 1.

**Open.**

- Guard the helper with the same condition as its callers, or move it into the
  `#[cfg(unix)]` module it belongs to; move the `#[cfg(unix)]` of the two
  release tests onto the test functions; and put the guard on the two tests of
  `tests/cli.rs` that use symlinks without one. A `cargo clippy --all-targets`
  run for a Windows target would prove it — the toolchain for
  `x86_64-pc-windows-msvc` is installed, but the MSVC SDK headers are not, so
  the cross-check does not compile yet (the simulation that found the defects
  replaced every `cfg(unix)` in `tests/` with `cfg(any())`).

### The artifact actions of the release run on a deprecated Node.js

Logged 2026-10-21 (the annotation of the release run of `v0.3.0`).

**Why.** The release workflow publishes its assets with
`actions/upload-artifact@v4` and collects them with
`actions/download-artifact@v4`. Both target Node.js 20, and the runners of
GitHub force them onto Node.js 24 — the run of `v0.3.0` carried the
annotation „Node.js 20 is deprecated". The run is green, so nothing is broken
today; a forced fallback is a courtesy, not a promise.

**Open.**

- Move both actions to the version that targets Node.js 24 as soon as it
  exists, and check the two workflows of this repository (the CI and the
  release) for the same pair. The action is used in the release only — the CI
  of this repository builds and tests, it publishes no artifact.
