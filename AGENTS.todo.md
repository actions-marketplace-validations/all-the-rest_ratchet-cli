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

### Follow-ups from the approved Verify-containment review

Logged 2026-10-23, from the independent review that approved PR #1. None of
these is release-blocking: the review verdict was APPROVED, and the four
security requirements of the Verify containment work are covered by code and
regressions.

**N2 — SPEC wording vs. implementation.** `SPEC.md:1125-1127` says "final
reparse points observed by Verify are refused", while `src/verify.rs`
(`check_directory_identity` 1349-1374, `create_snapshot_directories` 1044-1049)
refuses a reparse point anywhere in the resolved parent chain, at the target and
at the temporary file. On Windows, non-name-surrogate reparse points (e.g. cloud
placeholders) are therefore newly refused — fail-closed, but stricter than the
spec text. Align the wording.

**N3 — Duplicated Windows identity helper.** `high_resolution_windows_identity`
exists twice (`src/verify.rs:585`, `src/drift.rs:504`). The rejection of a
`LowRes` identity has a test in `drift` (`src/drift.rs:1737`) but none in
`verify`. Divergence risk, no security defect.

**N4 — Duplicated cleanup prefix.** `Guard::restore` (`src/verify.rs:703-709`)
wraps the `capture_command_directories` failures, and
`remove_created_directories` (`1275-1282`) already returns the same
"could not safely remove the created directories" prefix, so a failure can name
it twice. Cosmetic.

**N5 — Windows coverage of the outside-mutation regressions.** The two
outside-mutation regressions are Unix-only (`tests/cli.rs:6037`, `6092`); a
Windows counterpart using a junction (`cmd /c mklink /J`, no privilege needed)
is missing. In addition, `final_reparse_point_opens_are_refused`
(`src/verify.rs:2137-2161`) returns early when symlink creation is not
permitted and then passes without asserting anything.

**Accepted residual risk.** `write_snapshot` writes in place but only after a
restore recreated the file by rename, so a hardlink at the snapshot path is
replaced rather than written through; a swap after the restore falls under the
documented narrow race (SPEC 1129-1163).
