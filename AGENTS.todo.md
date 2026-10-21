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

### The moving action tag `v1` drifts

Logged 2026-10-21 (found by the review of the release workflow, before the
first release).

**Why.** The action is pinned by a tag of this repository (`v1` — a moving
major tag), and nothing moves it: every change to `action.yml` or `action/`
needs a `git tag -f v1 && git push -f origin v1`, and a forgotten move is
invisible — a consumer on `all-the-rest/ratchet-cli@v1` silently runs an old
action. The tag was pushed with the first release (`v0.1.0`) and moved with
the release of `0.2.0`; the drift is the open part — nothing does it
automatically.

**Open.**

- A workflow that moves the tag to the commit that last touched the action, or
  a check that fails when the tag and the action disagree. A project that
  pins the tag by its commit SHA — as the portal does, because its
  supply-chain contract allows no moving tag — is not affected.
