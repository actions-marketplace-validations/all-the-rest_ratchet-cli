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

### The drift gate creates the directory by the named path

Logged 2026-10-21 (found by the verification of the path containment work).

**Why.** The drift gate creates the directory of the generated file from the
**named** path (`src/drift.rs`). When that directory is behind a link to a
directory that does not exist yet, the creation fails with a misleading error
(`cannot create the directory docs of docs/generated.md: File exists (os
error 17)`) although the **resolved** directory lies inside the root and
could be created. The run fails closed — the generator could not have written
the file either — but the diagnosis points at the wrong place.

**Open.**

- Create the directory from the **resolved** path (the helper has to hand the
  resolved path to the caller, not only the verdict), or name the named path
  and the resolved one in the error.

### The moving action tag `v1` drifts

Logged 2026-10-21 (found by the review of the release workflow, before the
first release).

**Why.** The action is pinned by a tag of this repository (`v1` — a moving
major tag), and nothing moves it: every change to `action.yml` or `action/`
needs a `git tag -f v1 && git push -f origin v1`, and a forgotten move is
invisible — a consumer on `all-the-rest/ratchet-cli@v1` silently runs an old
action. The tag was pushed with the first release (`v0.1.0`) and moved by hand
with the releases of `0.2.0` and `0.3.0`; the drift is the open part — nothing
does it automatically.

**Open.**

- A workflow that moves the tag to the commit that last touched the action, or
  a check that fails when the tag and the action disagree. A project that
  pins the tag by its commit SHA — as the portal does, because its
  supply-chain contract allows no moving tag — is not affected.

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
