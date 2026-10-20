# AGENTS.md — ratchet-cli

`ratchet` is a standalone binary that enforces **ratchet gates**: quality gates
that compare a **count** against a **baseline** that may only move in one
declared direction, plus **drift gates** for generated files.

**Rules only, no decisions.** Durable decisions live in `SPEC.md`; open work in
`AGENTS.todo.md`.

## Language

English — code, comments, docs, commit messages.

## Green commands (the gate)

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

All three must pass before a commit. `cargo fmt` (without `--check`) fixes
formatting; never hand-format.

## Dependencies

Keep them few and justified. Every dependency is a supply-chain and a build-time
cost; a standard-library implementation is preferred over a crate for anything
small. No network access at runtime except the (optional, once-a-day) update
check.

## Commit convention

One Conventional Commit per self-contained change
(`feat|fix|docs|refactor|test|chore|perf` + optional scope). `git add` with
explicit paths — never `git add -A`. Verify with `git show --stat` before
committing.

## Design rules that are not negotiable

1. **The engine never mutates the tree** — except `--fix`, which exists for
   exactly that one purpose (regenerating generated files).
2. **Deterministic output**: sorted, no timestamps, no absolute paths, no
   machine names. The same tree produces the same bytes.
3. **A baseline is machine-written**, never hand-edited, and may only move in
   the direction the rule declares.
4. **A counter is project-specific, the mechanic is not.** The binary owns
   baseline handling, direction, monotonicity and reporting; the project owns
   the counting.
5. **Fail closed**: an unreadable config, a crashed counter or an empty
   selection is a failure, never a green.

## Release

`cargo build --release` + the GitHub Actions release workflow (tag `v*`).
Artifacts are per-OS/arch tarballs with a `checksums.txt`; `ratchet update`
verifies the checksum before replacing the binary.
