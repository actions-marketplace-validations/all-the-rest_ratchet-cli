# AGENTS.todo.md — open work

This file is a **log, not an order**: it records decisions and open work. Logging
is mandatory; working an entry off needs an explicit order. Verified-completed
entries are removed — the history lives in the commits.

The normative behaviour of the binary lives in `SPEC.md`; the rules for working in
this repo live in `AGENTS.md`. This file holds neither.

## Open

### Release step and changelog from the conventional-commit history

Logged 2026-10-20 (owner decision: "aus einer conventional commit history den
nächsten release step ermitteln können und auch die changelog generieren können
daraus").

**Why.** The version and the changelog are derived facts: both are already
determined by the commit history, and both are currently written by hand. A
hand-written version drifts from the history silently — the tag says `0.2.0`
while the history contains a breaking change.

**Decision.**

- **Semver + Keep-a-Changelog.** The next step is computed from the commits
  since the last tag: `feat` → minor, `fix` → patch, `!` in the subject or a
  `BREAKING CHANGE:` footer → major, `docs`/`refactor`/`chore`/`test`/`perf` →
  no step. A pre-1.0 project may configure the mapping, because `0.x` treats
  minor and patch differently.
- The changelog is written in **Keep-a-Changelog** sections (`Added` /
  `Changed` / `Deprecated` / `Removed` / `Fixed` / `Security`), grouped by the
  commit type, and appended to the project's existing `CHANGELOG.md` — the
  generator does not replace the file, it appends one release block.
- The engine owns the parsing, the mapping and the rendering; the project owns
  the version prefix and the changelog path. As everywhere else: the mechanic
  is not project-specific.

**Open.**

- `SPEC.md` gains a section for the release step (the current spec covers
  gates, baselines, drift and the self-update only — the release step is
  **not** part of v0).
- The binary gains `ratchet release [--from <tag>] [--dry-run]` printing the
  computed version and the changelog block, plus the write path.
- The tag lookup, the commit parsing and the Semver mapping need unit tests
  (conventional-commit subjects, `!` markers, footers, `0.x` handling).

### Security findings registry and gate

Logged 2026-10-20 (owner decision: "Befund-Registry + Gate").

**Why.** Two projects keep security findings in prose: the portal in
`features/security/2026-10-18-review-closure.md` (`SEC-01`…) with
`accepted-risks.md`, the website in `rules/10-ui-review-findings.md`. Neither
can answer "how many open high-severity findings are there, and did that number
grow?" — the number is not a number anywhere, so it cannot be gated.

**Decision.**

- A **JSON registry in the project** (`ratchet.findings.json`), entries with
  `id`, `severity`, `title`, `status` (`open` / `fixed` / `accepted`) and
  optional `regression_test` and `path`. The schema is project-neutral: both
  projects can use it, and the existing prose documents stay the human record.
- The gate counts the **open high/critical findings** against a baseline that
  may only **fall**. `accepted` findings do not count, but they stay visible in
  the report — an accepted risk is a decision, not a deletion.
- The registry is a **counter source** like the other artifacts, so the
  existing mechanic applies unchanged: the count comes from the registry, the
  baseline from `ratchet.baseline.json`, the direction from the rule.

**Open.**

- `SPEC.md` gains a section for the findings registry (the file format, the
  status semantics, the severity set, the interaction with `exclude`).
- The binary must read the registry as a counter source; the counting itself
  is a small, well-tested function.
- Both projects need the migration of their existing findings into the
  registry — a project-level task, not a tool task.

### Pre-release verification: run a command, fail on a change

Logged 2026-10-20 (owner decision: "man übergibt ihm einen befehl den er
ausführt und failt wenn sich da was verändert, das könnte ein pre release und
nicht immer gate sein").

**Why.** A benchmark is a measurement, not a defect class. It has no meaningful
"baseline that may only sink" — the value moves in both directions and only
matters at the moment of a release. Encoding it as a permanent gate would
either freeze a number or produce permanent noise.

**Decision.**

- A **pre-release check**, not a permanent gate: a command is executed, its
  output is compared with a **snapshot**, and a deviation **fails** the check.
  It runs in the release pipeline, not in the commit gate.
- The snapshot is a committed file (the same machine-written discipline as the
  baseline: never hand-edited, the diff is the review artifact). The direction
  is **not** restricted — a benchmark may legitimately move in either
  direction, and the human decides whether the deviation is acceptable. The
  tool reports the diff.
- The mechanic is the same as the drift gate (§7 of the spec) with a different
  comparison: the drift gate compares a *generated file* with its committed
  counterpart, the pre-release check compares a *command output* with a
  committed snapshot.

**Open.**

- `SPEC.md` gains a section for the pre-release check (the command, the
  snapshot, the comparison, the exit codes, the interaction with the drift
  gate).
- The binary gains the command, e.g. `ratchet verify --snapshot <path>`.

### Markdown report generator

Logged 2026-10-20 (owner decision: "Report-Generator + Drift-Gates").

**Why.** The report and the number must come from one source — otherwise they
contradict each other. A gate result that is only visible on the terminal is
not reviewable, and a status table in a Markdown file that is maintained by
hand drifts from the numbers it claims to report.

**Decision.**

- `ratchet report --format md` (or `--output <path>`) renders the gates, the
  findings and the metrics into **one Markdown file**; the existing drift gate
  then checks that the file matches the committed version, so every generated
  Markdown file is drift-checked like every other derived artifact.
- The generator is deterministic: sorted output, no timestamps, no absolute
  paths, no machine names. The same tree produces the same bytes.

**Open.**

- `SPEC.md` gains a section for the report format (the sections, the columns,
  the determinism rules).
- The binary gains the renderer; the drift gate already exists and only needs
  the generated file to be registered in the config.

### GitHub Action: validate and update the stand in CI

Logged 2026-10-20 (owner decision: "sie soll per cli verwendet werden aber auch
in die github ci gut integriert werden können. können wir da nativ einen step
bauen, den wir einbinden können? Es soll sowohl den Stand updaten können als
auch validieren ob der stand korrekt ist").

**Why.** The gates are worthless if nobody runs them, and running them by hand
in every repository is not a process — it is a habit, and habits are what the
ratchet mechanic exists to replace. Both projects that are to use the binary
(website, portal) run GitHub Actions, and the website repo deliberately has no
CI of its own today.

**Decision.**

- A **native GitHub Action** in this repo (`action.yml`), usable as
  `uses: reisi007/ratchet-cli@v1` in any project. Two modes:
  - **validate** — installs the binary and runs `ratchet check`; a failing
    gate fails the job. This is the commit/PR gate.
  - **update** — runs `ratchet baseline update` and commits the changed
    baseline. Only for a deliberate, scheduled or manually triggered run: the
    baseline is a reviewed artifact, and a bot that moves it silently on
    every push defeats the monotonicity check.
- The action does **not** decide the baseline direction, the tier or the
  rules — that stays in the project's `ratchet.config.jsonc`.

**Open.**

- `action.yml` in this repo, with the install path for the platform binaries
  (the release assets and their `checksums.txt` already exist for that).
- The workflow that builds and releases the action has to publish it with the
  binary releases (the release CI in the spec covers the binary; the action
  needs its own tag or a versioned release).
- The two consuming projects need a workflow that calls the action — a
  project-level task, not a tool task.

## Not open here

- The **skills repo** (`~/dev/agents-skills`) owns the `ratchet-gates` skill:
  the concrete CLI reference (`references/cli.md`) belongs there, and its
  checklist should point at the binary instead of describing the mechanic
  only. That is logged in the skills repo's own `AGENTS.todo.md`.
