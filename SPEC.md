# SPEC — `ratchet` v0

Normative specification for the `ratchet` binary. The implementation follows
this document; where the code and this document disagree, this document wins
and the code is the bug.

`ratchet` implements the mechanics of the `ratchet-gates` skill: gates that
compare a **count** against a **baseline** that may only move in one declared
direction, and **drift gates** for generated files.

---

## 1. Vocabulary

| Term | Meaning |
|---|---|
| rule | one gate, one defect class |
| class | the defect class the rule counts (e.g. *layer violation*) |
| incident | the incident that motivated the rule (see 3.2) |
| unit | what one count unit is (e.g. *call sites*) |
| direction | `ceiling` (count must not exceed the baseline) or `floor` (count must not fall below it) |
| count | the current measurement |
| baseline | the committed number the count is compared against |
| tier | `fast` (seconds, no infrastructure) or `slow` (real infrastructure) |
| drift | a generated file differs from what its generator produces |

## 2. Files

| File | Role |
|---|---|
| `ratchet.config.jsonc` | the only source of truth for rules. Also accepted: `ratchet.config.json`, `ratchet.config.mjs`, `ratchet.config.cjs`. |
| `ratchet.baseline.json` | machine-written baseline, one record per rule. Never hand-edited. |

Discovery order for the config: `--config <path>` → `./ratchet.config.jsonc`,
`./ratchet.config.json`, `./ratchet.config.mjs`, `./ratchet.config.cjs` →
walk up from the working directory to the git root. `--no-config` skips the
lookup (only valid together with an explicit `--config`).

The baseline path is fixed (`ratchet.baseline.json`, next to the config) unless
the config sets `"baseline": "<path>"` in a top-level object.

The baseline file must lie **inside the repository root**. A path outside it
cannot be compared with the committed version, and a baseline that cannot be
compared is a gate that does not gate. A config that points outside the root is
a configuration error.

## 3. Config format

The config is a **flat config** in the ESLint shape: an array of config
objects. Multiple objects are merged; a later object overrides an earlier one
for the same rule id. A single config object at the root is accepted as
shorthand for a one-element array.

```jsonc
[
  {
    "name": "noema",
    "tiers": {
      "fast": { "floor": 5 },
      "slow": { "floor": 1 }
    },
    "generated": [
      { "path": "docs/MISSING_FEATURES.md", "command": ["npm", "run", "docs:gaps"] }
    ],
    "rules": {
      "unwrapped-action-calls": {
        "class": "layer violation",
        "incident": "F-09: a server action was called without the shared error helper",
        "unit": "call sites",
        "direction": "ceiling",
        "tier": "fast",
        "count": { "command": ["node", "scripts/audit-inventory.mjs", "--print"] }
      },
      "unexpected-inner-html": {
        "class": "layer violation",
        "incident": "an unescaped HTML sink reached the renderer",
        "unit": "files",
        "direction": "ceiling",
        "tier": "fast",
        "count": { "file": "docs/_audit-raw.json", "path": "coherence.dangerouslySetInnerHTML" },
        "exclude": ["src/lib/markdown.ts", "src/lib/html-sanitize.ts"]
      }
    }
  }
]
```

### 3.1 Config object fields

| Field | Type | Meaning |
|---|---|---|
| `name` | string | optional label for this config object |
| `tiers` | object | per-tier settings (see 3.3) |
| `generated` | array | drift-gate entries (see §7) |
| `verify` | array | pre-release checks (see §15) |
| `rules` | object | rule id → rule object (see 3.2) |

### 3.2 Rule object fields

| Field | Type | Required | Meaning |
|---|---|---|---|
| `class` | string | yes | the defect class |
| `incident` | string | yes | the incident that motivated the rule (see below) |
| `unit` | string | yes | the count unit |
| `direction` | `ceiling` \| `floor` | yes | the allowed direction of the baseline |
| `tier` | `fast` \| `slow` | no (default `fast`) | which tier the rule belongs to |
| `count` | object | yes | how the count is produced (see 3.4) |
| `exclude` | string[] | no | glob patterns removed from the counted **items** (path-shaped exemptions only) |
| `files` | string[] | no | glob scope of the rule; without it the rule always runs |
| `description` | string | no | free text |

`exclude` requires a counter that yields **items**; a scalar counter plus
`exclude` is a configuration error.

**The `incident` field.** For a rule whose class has already occurred, the
incident names that concrete instance — it is what a later reader judges the
rule against when the count moves. For a **prevention rule** (a class whose
count is zero today), the incident says so plainly — for example *"prevention:
no instance in this project yet"* — instead of naming an incident that did not
happen. A gate that claims a history it does not have is worse than no gate:
the next reader cannot tell a real regression from a remembered one.

### 3.3 Tiers

```jsonc
{ "tiers": { "fast": { "floor": 5 }, "slow": { "floor": 1 } } }
```

`floor` is the minimum number of checks that must **execute** in that tier.
Below the floor the tier fails. A tier that executes **zero** checks always
fails — an empty selection is never a pass.

A check that ran and failed counts as executed: it fails anyway (exit 2), so
the floor only says how many checks the tier has to run. A check that was
never selected does not count — that is the empty selection, and it fails.

A **check** is a **rule**. The drift gates are not counted: they are gates over
the *generated* files, and the report — which is a generated file — cannot be
part of the count that decides whether the report is coherent. A tier that
declares a floor therefore needs that many rules.

The run as a whole fails when **no check ran at all** — the empty selection is
never a pass, whether the selection consists of rules, of drift gates, or of
both.

The floor of **every tier that runs** is enforced. Without `--tier`, every
declared tier's floor applies — the floor is what stops a tier from being
quietly narrowed, and a narrowing that only shows up when somebody selects
the tier by hand is a narrowing that survives.

### 3.4 Counter

Exactly one of:

```jsonc
{ "command": ["node", "scripts/count.mjs", "--json"] }
{ "file": "docs/_audit-raw.json", "path": "coherence.unwrappedCallSites" }
{ "file": "docs/_audit-raw.json", "path": "coherence.unwrappedCallSites", "generate": ["node", "scripts/audit-inventory.mjs"], "output_arg": "--output" }
{ "registry": "ratchet.findings.json", "severities": ["critical", "high"], "statuses": ["open", "in-progress"] }
```

* `command` — an argv array. **No shell** is involved; the arguments are passed
  through verbatim. The working directory is the repository root.
* `file` — a JSON file, optionally with a dotted `path` into it. Without
  `path` the root value is used.
* `generate` (only with `file`) — an argv array that regenerates the artifact.
  The binary runs it **before** reading the count and appends the **temporary
  output path** to the argv (`output_arg` is mandatory). The tree is never
  touched, and the count always comes from the freshly generated artifact — a
  count read from a stale artifact is a gate that lies.

  `generate` without `output_arg` is a configuration error: a generator that
  cannot write to a given path would have to write into the tree, and the tree
  is not the engine's to write. The in-place mode exists only for the drift
  gate (§7), which owns a verified restore.

  Without `generate`, the artifact is read as it is; the project is then
  responsible for its freshness. `output_arg` without `generate` is a
  configuration error — a silently ignored argument is a rule that does not do
  what it says.

The value the counter produces may be:

| Value | Meaning |
|---|---|
| integer | the count |
| array of strings | the items; the count is the number of items after `exclude` |
| object `{"count": N}` | the count |
| object `{"count": N, "items": [...]}` | the count and the items |

For the `command` form the stdout is parsed: trimmed, then tried as an integer,
then as JSON in the shapes above. Anything else is a counter error.

### 3.5 Unknown fields

An unknown field in the config, in a rule, or in a counter is a configuration
error. Typos fail loudly instead of silently disabling a gate.

### 3.6 JavaScript config

`ratchet.config.mjs` / `.cjs` is supported and is **data only**: it exports the
same JSON shape (default export). The engine is a standalone binary, so the
config cannot carry rule *functions* — a rule counts through `command` or
`file`.

The file is evaluated by `node`. If `node` is not on `PATH`, the error message
names the alternative (`ratchet.config.jsonc`). The bridge evaluates the module
and reads its default export; the result must be JSON-serialisable.

Known limitation: a repository path containing a backslash cannot be loaded
through the JS bridge (node rejects the percent-encoded backslash in a file
URL). The failure is closed — the error names the cause. Use
`ratchet.config.jsonc` in such a checkout.

### 3.7 Glob patterns

`*` (any run of characters except `/`), `**` (zero or more path segments),
`?`, and `[...]` classes. Matched against repository-relative paths with `/`
as the separator.

A pattern that ends in `/**` also matches the directory itself: `src/**`
matches `src` and everything below it. A scope that selects nothing is a
failure, not a pass — the rule that would have run is not silently skipped.

## 4. Baseline file

```json
{
  "version": 1,
  "rules": {
    "unwrapped-action-calls": { "value": 11, "reason": "F-09 remainder, tracked in P1.2" }
  }
}
```

* Machine-written by `ratchet baseline init`, `ratchet baseline update` and
  `ratchet baseline raise`.
* Never hand-edited.
* `reason` is optional and only present when one was given. It does more than
  explain a paydown: it is what makes a **deliberate move against the declared
  direction** reviewable — the reason travels with the record, so the diff on
  the baseline file carries it.
* Rules are sorted by id — the file is a stable diff.

## 5. Commands

```
ratchet [check] [paths…] [options]
ratchet count [--rule <id>] [--format <stylish|json>]
ratchet baseline init [--rule <id>] [--reason <text>]
ratchet baseline update [--rule <id>] [--reason <text>]
ratchet baseline path
ratchet baseline raise --rule <id> --value <n> --reason "<text>"
ratchet drift [--path <path>] [--format <stylish|json>]
ratchet verify [--name <name>] [--update]
ratchet report [--output <path>] [--format md]
ratchet init
ratchet update
```

The default command is `check`.

### 5.1 Options

| Option | Meaning |
|---|---|
| `--config <path>` | use this config file |
| `--no-config` | do not look for a config (requires `--config`) |
| `--format <stylish\|json\|md>` | output format, default `stylish` (`md` only for `report`) |
| `--output <path>` | write the report to this file (relative to the repository root) — a `report` option |
| `--fix` | apply the fixable: regenerate generated files — a **`check`/`drift`** option, not a global one |
| `--tier <fast\|slow>` | run only the rules of that tier — a `check` option |
| `--rule <id>` | restrict to one rule |
| `--ignore <pattern>` | glob of paths to exclude from the run (repeatable) |
| `--name <name>` | run only this check — a `verify` option |
| `--update` | write the snapshot instead of comparing it — a `verify` option |
| `--debug` | print the resolved config and every counter invocation |
| `--quiet` | only print failures |
| `--version`, `--help` | |

`--ignore` takes glob patterns of paths to exclude from the selection. A rule
whose scope does not intersect the remaining paths is skipped. Ignoring a path
never turns a failing gate green: a rule that would have run on the ignored
path still runs on the rest.

`--ignore` applies to the **rules' path scope**. It does not exclude a drift
gate: a generated file is registered deliberately, and a pattern that happens to
match it must not silently disable the gate.

### 5.2 `check`

1. Load and validate the config.
2. For every rule: run the counter → count (+ items).
3. Compare the count with the baseline:
   * `ceiling` — pass iff `count ≤ baseline`
   * `floor` — pass iff `count ≥ baseline`
   * no baseline → status `missing-baseline`, fail, hint: `ratchet baseline init`
4. Monotonicity — compare the **working-tree** baseline with the **committed**
   baseline (`git show HEAD:<baseline>`):
   * `ceiling` — the baseline must not have **increased** → `baseline-raised`, fail
   * `floor` — the baseline must not have **decreased** → `baseline-lowered`, fail
   * no git, or the file is not in `HEAD` → skipped, reported as a note, not a failure
   * the committed baseline exists but cannot be parsed → **operational error**
     (exit 2). A machine-written baseline that does not parse must not silently
     disable the gate.

   When the monotonicity check fails, it takes precedence over the count
   comparison in the report: the baseline is the invalid artifact, and the
   count comparison is reported only as an additional note.

   The monotonicity check accepts a move against the declared direction **only**
   when the baseline record carries a reason. A raise without a reason is
   rejected like any other forbidden move; a raise with a reason is allowed,
   because the reason is in the file and therefore in the diff — the change is
   visible, deliberate and reviewable on its own.
5. Tiers — `--tier` selects one tier. The number of **executed** checks in the
   selected tier must be ≥ the tier's `floor`, and must be > 0. Zero executed
   checks in the selected tier → fail ("empty selection").
6. Drift — the drift gates run as part of `check` (§7); `ratchet drift` runs
   only them. A drift is a **gate failure** (exit 1) — a stale committed file
   is the defect — while a generator that could not be run is an **operational
   error** (exit 2).
7. Exit code: `0` all green, `1` at least one gate failed, `2` operational error.

`paths…` restrict the run to the rules whose counter is affected by those
paths. A rule without a path scope always runs. A rule may declare
`"files": ["src/**"]` to scope it; the rule is skipped when the selected paths
do not intersect its scope. A path that does not exist is an operational error
(exit 2) — a typo must not silently narrow the run into a green one.

### 5.3 `count`

Prints the current count per rule (no baseline comparison). Exit `0`, or `2` on
an operational error.

### 5.4 `baseline init`

Writes the **current count** as the baseline for every rule that has no
baseline yet. Existing baselines are never touched. `--rule` restricts it to
one rule. Touches only the baseline file.

### 5.5 `baseline update` (the paydown)

Moves the baseline to the current count, **only in the allowed direction**:

* `ceiling` — allowed iff `count ≤ current baseline`; otherwise refused:
  *"refusing: count 15 > baseline 12 — a ceiling may only decrease"*.
* `floor` — allowed iff `count ≥ current baseline`; otherwise refused
  analogously.

Rules that may not move are reported as refused and are **not** modified; the
command then exits `1` so the refusal is noticed. `--rule` restricts it to one
rule. `--reason <text>` is stored in the baseline record. Touches only the
baseline file. Never runs automatically.

A move **against** the declared direction is possible only as its own
deliberate command:

```
ratchet baseline raise --rule <id> --value <n> --reason "<text>"
```

It writes the new value **with the reason** and touches nothing else. A raise
without a reason is refused like any other forbidden move. `baseline update`
keeps its shape: it moves only in the allowed direction and refuses the other.

`ratchet baseline path` prints the repository-relative path of the baseline
file — the path the binary itself uses. It is the answer to the question "which
file is my baseline?", and it is what a script outside the binary must ask
instead of re-implementing the config's resolution.

### 5.6 `init`

Writes a starter `ratchet.config.jsonc` (one example rule) if no config
exists. Never overwrites an existing config.

### 5.7 `update`

See §9.

## 6. Output

### 6.1 `stylish` (default)

```
ratchet — 3 rules, 2 failed

  FAIL  missing-baseline              no baseline for module-test-coverage
  FAIL  unexpected-inner-html         7 > 5  (files)
  PASS  unwrapped-action-calls        9 ≤ 11  (call sites)

NOT COHERENT — 2 of 3 gates failed
```

The rules are sorted by id, the label column is the longest label plus six
characters plus a two-space separator, and the unit is separated by two spaces.
The example is the byte-exact output of the binary — the test
`the_stylish_output_is_the_documented_one` compares the whole stdout against
it, so the document cannot drift away from the implementation.

Every path in the output is **relative to the repository root**. An absolute
path, a machine name or a timestamp makes the same tree produce different
bytes — the determinism rule is not negotiable.

### 6.2 The report and the diagnostic channel

The **report** — the stylish output and the JSON document — contains **only
ratchet's own words**: a rule id, a status, a count, a baseline, a path
relative to the repository root, the exit status of a failed subprocess.

The output of a counter, a generator or `node` is **never relayed into the
report**. A subprocess that prints a timestamp, a hostname or an absolute path
would make the same tree produce different bytes, and no filter can be relied
on to catch every form of it.

A failed subprocess is therefore reported as *what* it was — **which rule and
which exit status** — not as *what it printed*. The command line is **not** part
of the report: an argument may be an absolute path, and §6.1 requires every path
in the report to be relative. The argv belongs to the diagnostic channel.

The same separation applies to a counter whose output cannot be parsed: the
report says that the counter did not print a count; the offending output goes
to the diagnostic channel.

The **diagnostic channel** is `--debug`: it prints the resolved config, every
counter invocation and the raw output of every subprocess — including the argv
and everything a subprocess printed. It is a transcript for one human reading
one run and is **explicitly exempt from the determinism rule**: two runs with
`--debug` may differ. Without `--debug`, two runs over the same tree produce
byte-identical output.

### 6.3 `json`

```json
{
  "schema": 1,
  "ok": false,
  "summary": { "rules": 3, "failed": 1, "executed": 3, "generated": 1, "drifted": 0 },
  "rules": [
    {
      "id": "unwrapped-action-calls",
      "status": "ok",
      "count": 9,
      "baseline": 11,
      "direction": "ceiling",
      "unit": "call sites",
      "class": "layer violation",
      "tier": "fast"
    }
  ],
  "generated": [{ "path": "docs/MISSING_FEATURES.md", "status": "ok", "diff": null }],
  "notes": ["the tier `slow` executed 2 checks, the floor is 5"]
}
```

Rule status values: `ok`, `over-baseline`, `under-floor`, `baseline-raised`,
`baseline-lowered`, `missing-baseline`, `counter-error`.
Generated status values: `ok`, `drift`, `new`.

A rule entry may carry a `detail` field: the reason for a status that is not
`ok`. It is absent when there is nothing to say. A machine consumer must be
able to learn **why** a rule failed without parsing the human output.

The `notes` array carries the same notes the human output prints on stderr —
the tier floor, the skipped monotonicity comparison, a skipped drift gate, a
baseline that was raised deliberately (with its reason). It is empty when there
is nothing to say. A machine consumer must be able to learn why the run is not
green **without** the note being only on stderr: a report that says `ok: false`
and gives no reason is a report that cannot be acted on.

## 7. Drift gates

```jsonc
{ "generated": [ { "path": "docs/MISSING_FEATURES.md", "command": ["npm", "run", "docs:gaps"], "output_arg": "--output" } ] }
```

A `generated[]` entry may declare a `tier` (`fast` | `slow`, default `fast`).
`--tier fast` skips the slow generators: a generator that needs a database, a
browser or the network belongs to the slow tier, and a fast tier that waits for
it is a fast tier that is not fast.

The generator must be able to write to a **given path**:

* with `output_arg` — the binary appends the temporary output path to the argv.
* without `output_arg` — the generator writes the file in place. The binary
  then owns a **verified restore**:
  1. snapshot the original bytes **and the file mode**;
  2. run the generator;
  3. read the produced bytes;
  4. restore: write the snapshot to a **temporary file in the same directory**,
     give it the original mode, and **rename** it over the target — a rename
     replaces a symlink instead of writing through it, and it does not need
     write permission on the target itself. If the target **is** a symbolic
     link, the restore writes the file **behind** the link and recreates the
     link: a rename over the link would replace it with a regular file and
     change the tree, and the tree must not change;
  5. **verify** the restore (read the bytes back and compare);
  6. a directory the generator created and left empty is removed.

  If the restore fails, the run ends with an **operational error** that names
  the file and says that the working tree was modified. A swallowed restore
  error is a mutated tree that nobody was told about.

The comparison is against the **committed** file (`git show HEAD:<path>`), not
against the working-tree copy. The diff is a unified diff (committed →
produced) with repository-relative paths.

A file that is not in `HEAD` yet has the status `new` — informational, not a
failure.

`--fix` writes the produced content to the target path and reports what it
did. The gate stays **failed**: the committed file is still stale — the gate
measures the **committed** artifact, and it only turns green once the fix is
committed. A run with `--fix` in a pipeline is therefore never green against a
stale HEAD; that is deliberate, not a bug. `--fix` touches **only** the
generated files, never hand-written text.

## 8. Git

* Repository root: `git rev-parse --show-toplevel`; fallback: the directory of
  the config file.
* Committed baseline: `git show HEAD:<baseline-path>`.
* Committed generated file: `git show HEAD:<path>`.
* No git, or not a repository → the committed comparisons are skipped, a note
  is printed, the gate still runs.
* A committed file that exists but cannot be parsed is an **operational error**
  (exit 2), not a skip — a broken reference must not disable a gate.

## 9. Self-update

* `ratchet update` downloads the newest release asset for the platform from the
  GitHub Releases API, verifies the SHA-256 against `checksums.txt`, replaces
  the binary atomically and prints the new version.
* A release publishes **two** assets per platform: the **archive**
  (`ratchet-<version>-<os>-<arch>.tar.gz`, `.zip` on windows) for the manual
  install, and the **raw binary** (`ratchet-<version>-<os>-<arch>`, `.exe` on
  windows) for the self-update. The self-update takes the binary: unpacking an
  archive would need an inflate implementation, and the binary has no runtime
  dependency. Both names come from one stem, and the release workflow and the
  binary are checked against each other so the two cannot drift.
* The replacement: on unix the new binary is written next to the old one and
  **renamed** over it (the running process keeps its inode); on windows a
  running executable cannot be replaced — the new binary is written next to it
  and the command says what to do. Doing nothing silently is the worst of the
  three.
* A **wrong checksum** refuses the update and leaves the binary untouched.
* The version comparison is hand-written for `MAJOR.MINOR.PATCH` (a pre-release
  suffix is ignored, a missing field counts as zero). A malformed version
  fails closed: the update is refused, nothing is downloaded, nothing is
  touched.
* The version comes from the manifest (`Cargo.toml`), never from a string in
  the code, and the release workflow **checks the tag against the manifest**: a
  tag `v0.2.0` with a manifest that says `0.1.0` fails the release.
* An update **check** runs at most once per 24 h. The timestamp lives in
  `$XDG_CACHE_HOME/ratchet/update-check` (fallback `~/.cache/ratchet/update-check`).
  It is **synchronous but bounded** (2 s for the whole request) and it happens
  at most once a day; it is silent on every failure and it never changes the
  exit code. "Never blocks" means: it never decides the outcome and it never
  fails the run.
* Disable with `--no-update-check` or `RATCHET_NO_UPDATE_CHECK=1`. The CI sets
  the variable, so the CI runs stay hermetic.

## 10. Exit codes

| Code | Meaning |
|---|---|
| `0` | green |
| `1` | at least one gate failed |
| `2` | operational error: invalid config, missing config, crashed counter, unreadable file |

`ratchet report` is the exception: it exits **0** when it produced a report
(§14) — the report is a *generator*, and a generator that failed because the
run is red would fail the gate that invoked it. The run's verdict is in the
report's content. `ratchet check` is the gate.

## 11. Non-goals of v0

* No Node API and no npm plugin registry.
* No rule *functions* in the config — the engine is a runtime-free binary.
* No automatic fix of violations; `--fix` only regenerates generated files.
* No automatic baseline change; `baseline update` is always explicit.

## 12. The GitHub Action

A native action in this repository (`action.yml`), usable as
`uses: all-the-rest/ratchet-cli@v1` in any project.

**Two modes** (the input `mode`):

| mode | what it does | when |
|---|---|---|
| `validate` | installs the binary, runs `ratchet check`; a failing gate fails the job | every push and every pull request |
| `update` | installs the binary, runs `ratchet baseline update` and **commits** the changed baseline | a deliberate, scheduled or manually triggered run |

An unknown mode **fails the job**: a mode that runs neither step would leave
the job green without having done anything.

**The inputs:** `mode`, `version` (the binary version the action installs),
`config`, `tier` (the tier of the **validate** mode — `ratchet baseline update`
has no tier selection, so the update mode ignores it), `args`,
`baseline-reason`.

**The install:** the **raw binary** asset of the release (no `tar`, no `unzip`,
no platform-specific unpack step), verified against the release's
`checksums.txt` — `sha256sum` or `shasum -a 256`, whichever the runner has. A
missing checksum or a mismatch **fails the step**; nothing is installed from an
unverified download.

**The naming:** the asset name is defined **once**, in the binary. The action
and the release workflow mirror it, and a test substitutes their templates and
compares the result with the binary's function for every platform — a renamed
asset fails the test instead of the action.

**The update mode** commits the baseline file and nothing else, with the
identity `ratchet[bot]`, and it **never pushes** — the push is the calling
workflow's decision. It needs a `baseline-reason` (an empty one fails the step)
and it moves the baseline only in the declared direction: a deliberate raise
stays a human decision, because a bot that raises on every push would defeat
the monotonicity check.

The action **asks the binary** which file is the baseline (`ratchet baseline
path`) — it never guesses it from the config. A script that re-implements the
config's resolution answers a different question than the binary, and a wrong
answer is a commit of the wrong file. The action also **refuses a dirty
working tree**: with a clean tree, the file the binary changed is the baseline
file, and the commit contains it and nothing else.

**The permissions** are the workflow's: the action cannot grant them. The
`validate` mode needs `contents: read`, the `update` mode `contents: write`.

**Two version lines:** the action is pinned by a tag in this repository, the
binary by the action's `version` input. A project pins the action; the action
pins the binary — the project never downloads a binary itself.

## 13. The findings registry

A JSON registry in the project: `ratchet.findings.json`.

```json
{
  "version": 1,
  "findings": [
    {
      "id": "SEC-01",
      "title": "org admins reassign foreign users",
      "severity": "critical",
      "status": "fixed",
      "status_since": "2026-10-19",
      "owner": "…",
      "ticket": "D-12",
      "regression_test": "OrgAdminDelegationBoundaryTest",
      "path": "backend/src/…",
      "note": "…"
    }
  ]
}
```

### The fields

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | stable and unique; minted once where the source has none (`UI-001`, …) |
| `title` | yes | one line |
| `severity` | yes | one of the canonical levels |
| `status` | yes | one of the canonical statuses |
| `status_since` | no | the date the status was reached |
| `owner` | yes, unless the status is `fixed` | who owns the finding |
| `ticket` | yes, unless the status is `fixed` | the decision, the review round or the issue that tracks it |
| `regression_test` | no | the test that proves the fix |
| `path` | no | where the defect lives — a **pointer** for a reader, not a path the tool reads |
| `note` | no | free text |

An `owner` and a `ticket` are mandatory for everything that is not `fixed`: a
closed finding has its commit and its regression test, an open one has nobody
— a graveyard entry.

### The vocabulary

The levels and the statuses are **not** decided here: they are defined once in
the `ratchet-gates` skill
(`~/dev/agents-skills/.agents/skills/ratchet-gates/references/vocabulary.md`):

* the levels `critical`, `high`, `medium`, `low`, `info`;
* the statuses `open`, `in-progress`, `fixed`, `accepted`, `wontfix`.

An unknown level or status is a **hard error** (exit 2, with the offending
entry named), never a fallback. A registry that cannot be read is not a gate
that reports zero findings — it is an error.

### The counting rule

The count is the number of findings whose status **counts**: `open` and
`in-progress`. `accepted` and `wontfix` do not count but stay visible — a
decision is not a deletion. `fixed` is resolved.

**Which levels count is named by the rule**, not by the engine:

```jsonc
"count": { "registry": "ratchet.findings.json", "severities": ["critical", "high"], "statuses": ["open", "in-progress"] }
```

The rule's lists are **authoritative** when they are present: `statuses` names
the statuses that count, `severities` the levels that count. Without them, every
level counts and the counting statuses of the vocabulary count. An empty list is
a configuration error — a gate that counts nothing is green forever.

A rule with `direction: "ceiling"` and `baseline: 0` **is** the boolean gate
("no open finding"): the same mechanic, no second kind of gate.

### The registry is a view

The prose document (the security review closure, the UI review log) stays the
**human record**. The registry is the machine-readable view of it. The two are
migrated in the same commit and must not disagree about the severity of a
finding — a status written into the severity field destroys the severity, and
the migration has to recover it from the row.

## 14. The Markdown report

`ratchet report --format md [--output <path>]` renders a run into **one
Markdown file**.

The report and the number must come from **one source** — otherwise they
contradict each other. A gate result that is only visible on the terminal is
not reviewable, and a status table maintained by hand drifts from the numbers
it claims to report.

**The sections:**

1. a summary (the rules, the failed, the executed) **and the run's verdict**;
2. one row per rule (id, class, direction, tier, count, baseline, status, and the
   **detail** — the reason a status is not `ok`);
3. for every rule that counts a **registry**: the findings that count (id,
   severity, status, title, owner, ticket) — the report says *which* findings
   are open, not only how many;
4. the run's **notes** (the tier floor, the skipped comparisons).

**The report renders the gates that do not depend on the report.** The drift
gates are not in it: a report that contained the result of the comparison with
itself would be a regress, and the report is exactly what the drift gate
compares. The drift gates' verdicts belong to the `check` — and to the diff
the gate prints. The report is a snapshot of the **rules**: their counts, their
baselines and the findings behind them.

**The report of a red run is not green.** The report carries the run's
**verdict** and its **notes** (the tier floor, the skipped comparisons). A
committed artifact that says "0 failed" while the run is red is exactly the
drift this feature exists to prevent.

**The report is drift-checked like every other derived artifact:** the project
registers it in `generated[]`, and the drift gate compares it with the
committed version. The tool checks its own output — and because the report
contains no drift verdict, that comparison has no regress.

**The report is deterministic:** sorted output, no timestamps, no absolute
paths, no machine names. The same tree produces the same bytes — otherwise the
drift gate would report a difference that does not exist.

## 15. The pre-release check

A command is executed, its output is compared with a committed snapshot, and a
deviation fails the check.

A **benchmark is a measurement, not a defect class**. It has no meaningful
"baseline that may only sink" — the value moves in both directions and only
matters at the moment of a release. Encoding it as a permanent gate would either
freeze a number or produce permanent noise. So the pre-release check is **not a
ratchet**: it has no direction.

```jsonc
{
  "verify": [
    {
      "name": "benchmark",
      "command": ["cargo", "bench", "--", "--output", "<out>"],
      "snapshot": "snapshots/benchmark.txt"
    }
  ]
}
```

* The **snapshot** is a committed file, machine-written, the same discipline as
  the baseline: never hand-edited, the diff is the review artifact.
* The command writes to a **temporary path** (like the drift gate's
  `output_arg`), so the tree is not modified by the check.
* The comparison is a diff, and the report shows it — a benchmark that moved by
  3 % is not a failure of the code, it is a number a human looks at.
* It runs in the **release pipeline**, not in the commit gate: it needs the
  infrastructure the release needs, and a permanent gate over a measurement
  would be noise.

The mechanic is the drift gate (§7) with a different comparison: the drift gate
compares a *generated file* with its committed counterpart, the pre-release
check compares a *command output* with a committed snapshot.

**The command.** `ratchet verify [--name <name>] [--update]`:

* the **statuses** are `ok`, `drift` and `new` — the same vocabulary as the
  drift gates, for the same reasons (a snapshot that is not committed yet is
  `new`: informational, not a failure);
* the **exit codes**: `0` when every entry is `ok` or `new`, `1` when a
  snapshot differs, `2` on an operational error (the command could not be
  started, it failed, it wrote nothing, it wrote more than one file — the
  output would be a guess, the committed snapshot could not be read, the
  snapshot path is outside the repository root, the snapshot is not a regular
  file, the restore of the snapshot failed). **An empty selection is not a
  pass**: a `verify` without an entry exits `1` — the same rule as the drift
  command;
* `--name` runs a single entry; an unknown name is an operational error;
* `--update` **writes** the snapshot (the tool writes it, a human never edits
  it) and the check stays **failed** while the committed snapshot is stale —
  the same rule as the drift gate's `--fix`: the file is written, the detail
  says "commit it", and it turns green after the commit;
* the JSON document reuses the **check document's envelope** (§6.3) with the
  `generated` array replaced by a `verify` array.

**The snapshot is a regular file.** A symbolic link is refused with a named
error: the committed content of a link is its target, and comparing the
*output* with the target's content would say nothing about the output. (The
drift gate resolves links, because there the *file* is the subject; the two
gates have different subjects and say so.)

**The placeholder `<out>`** in the command is replaced by the temporary path
**wherever it appears in an argument** — not only as a whole argument. A
command that writes `--output=<out>` must write to the temporary file, or the
engine would compare the command's stdout with the snapshot and report a
green that means nothing.

**The snapshot is restored after the command.** The engine owns exactly one
file — the snapshot — and it gives it back unchanged:

1. take the state of the file: its **bytes**, its **mode** and its **time**;
2. touch the file with a time far enough in the past, so that a write of the
   **same bytes** is still visible (the marker);
3. run the command;
4. restore: the bytes, the mode **and the time** — and **verify** the
   restore;
5. a directory the command created and left empty is removed.

A command that rewrites the snapshot with the same bytes is detected through
the marker and restored like any other write. If the restore fails, the run
ends with an **operational error** that names the file and says that the
working tree was modified — the same rule as the drift gate (§7). A swallowed
restore error is a mutated tree that nobody was told about.
