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

A **check** is a rule **or a drift gate** that runs in the tier. A tier that
declares a floor therefore needs that many rules and generators: a tier whose
promised checks are all missing is exactly the narrowing the floor exists to
catch.

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
ratchet baseline raise --rule <id> --value <n> --reason "<text>"
ratchet drift [--path <path>] [--format <stylish|json>]
ratchet init
ratchet update
```

The default command is `check`.

### 5.1 Options

| Option | Meaning |
|---|---|
| `--config <path>` | use this config file |
| `--no-config` | do not look for a config (requires `--config`) |
| `--format <stylish\|json>` | output format, default `stylish` |
| `--fix` | apply the fixable: regenerate generated files |
| `--tier <fast\|slow>` | run only the rules of that tier |
| `--rule <id>` | restrict to one rule |
| `--ignore <pattern>` | glob of paths to exclude from the run (repeatable) |
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

## 11. Non-goals of v0

* No Node API and no npm plugin registry.
* No rule *functions* in the config — the engine is a runtime-free binary.
* No automatic fix of violations; `--fix` only regenerates generated files.
* No automatic baseline change; `baseline update` is always explicit.
