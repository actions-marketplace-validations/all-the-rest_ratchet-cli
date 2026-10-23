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

### Verify restore and cleanup can escape the repository

Logged 2026-10-22 (found by the final milestone review before `v0.3.2`).

**Why.** Verify stores snapshot and traversal-directory paths before running a
configured command, then restores/removes by pathname without rechecking that
ancestors still resolve inside the repository or still identify the original
objects. A reproduced command replaced `sub` with a symlink to an outside
directory: Verify wrote the committed snapshot to `outside/snapshot.txt`; with
`sub/x/../snapshot.txt`, cleanup also removed a pre-existing empty
`outside/x`.

**Open.** Revalidate repository containment and the initially authorized
target/parent identity immediately before every post-command snapshot read,
restore write/removal/time operation, verification read, and traversal-directory
inspection/removal. Fail closed when an ancestor or object changed. Add
regressions proving outside files are neither created nor overwritten and
outside directories are not removed. Preserve linked-snapshot and normal
cleanup behavior.

**Decision 2026-10-22.** The user explicitly accepts a narrow race between the
final identity-and-containment validation and each pathname-based Verify
operation. A `cap-std` prototype was reviewed and rejected for this release:
Windows cannot restore an absolute in-root symlink exactly, and the prototype
also introduced rollback/cleanup regressions. The user selected Sol's
path-revalidation plan; retain existing dependencies and exact symlink behavior.

**Decision 2026-10-23.** To address the independently reported Windows
path-identity/handle mismatch without adding `unsafe` or a dependency, the user
authorized an isolated prototype of Windows handle pinning: retain snapshot
ancestor directory handles without `FILE_SHARE_DELETE`, and use post-command
file handles that deny delete sharing and do not follow a final reparse point.
The behavior trade-off is that a Verify command cannot rename/delete a tracked
snapshot-parent directory on Windows. Continue toward release only after this
approach passes independent review and green CI.

**Windows CI status 2026-10-23.** PR #1's run `39942541321` passed Ubuntu and
macOS but failed on Windows in `tests/cli.rs`: 208 passed, 10 failed. The
failures cover Windows file-time updates (`Access is denied`), Unix-style
absolute-path expectations, generator/counter behavior, and patch application.
The action/path follow-up is on `fix/verify-path-revalidation`; the Windows CI
gate remains unresolved. Do not release `v0.3.2` or re-pin adopters until all
three CI jobs pass and the milestone review is approved.

**Windows CI follow-up 2026-10-23.** Run `39933128158` compiled the Windows
Verify code and passed all 124 library tests, but 19 of 39 action integration
tests failed. The current correction sets inherited `PATH` explicitly for
every Bash test invocation and computes the baseline's repository-relative
name against the canonical repository root, matching the resolved baseline's
Windows path spelling. These changes are not validated on Windows yet; keep
release and adopter re-pins blocked until a fresh Ubuntu/macOS/Windows PR run is
green.

**Latest CI 2026-10-23.** Run `39937084759` for commit `970e2e8` passed
Ubuntu and macOS. Windows reached `tests/cli.rs`, where 208 tests passed and 10
failed. Failures include Windows file-time updates returning access denied,
Unix-style absolute-path expectations, generator/counter behavior, and the
patch-application test. The previous action-harness failure was no longer the
reported failing test binary. No fixes for these new failures are included
here; Windows CI remains red, so release and adopter re-pins remain blocked.
