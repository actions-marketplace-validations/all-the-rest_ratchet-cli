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
| incident | the concrete past incident that proves the class exists |
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

## 3. Config format

The config is a **flat config** in the ESLint shape: an array of config
objects. Multiple objects are merged; a later object overrides an earlier one
for the same rule id.

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
| `incident` | string | yes | the concrete past incident |
| `unit` | string | yes | the count unit |
| `direction` | `ceiling` \| `floor` | yes | the allowed direction of the baseline |
| `tier` | `fast` \| `slow` | no (default `fast`) | which tier the rule belongs to |
| `count` | object | yes | how the count is produced (see 3.4) |
| `exclude` | string[] | no | glob patterns removed from the counted **items** |
| `files` | string[] | no | glob scope of the rule; without it the rule always runs |
| `description` | string | no | free text |

`exclude` requires a counter that yields **items**; a scalar counter plus
`exclude` is a configuration error.

### 3.3 Tiers

```jsonc
{ "tiers": { "fast": { "floor": 5 }, "slow": { "floor": 1 } } }
```

`floor` is the minimum number of checks that must **execute** in that tier.
Below the floor the tier fails. A tier that executes **zero** checks always
fails — an empty selection is never a pass.

### 3.4 Counter

Exactly one of:

```jsonc
{ "command": ["node", "scripts/count.mjs", "--json"] }
{ "file": "docs/_audit-raw.json", "path": "coherence.unwrappedCallSites" }
```

* `command` — an argv array. **No shell** is involved; the arguments are passed
  through verbatim. The working directory is the repository root.
* `file` — a JSON file, optionally with a dotted `path` into it. Without
  `path` the root value is used.

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

### 3.7 Glob patterns

`*` (any run of characters except `/`), `**` (any number of path segments),
`?`, and `[...]` classes. Matched against repository-relative paths with `/`
as the separator.

## 4. Baseline file

```json
{
  "version": 1,
  "rules": {
    "unwrapped-action-calls": { "value": 11, "reason": "F-09 remainder, tracked in P1.2" }
  }
}
```

* Machine-written by `ratchet baseline init` and `ratchet baseline update`.
* Never hand-edited.
* `reason` is optional and only present when one was given.
* Rules are sorted by id — the file is a stable diff.

## 5. Commands

```
ratchet [check] [paths…] [options]
ratchet count [--rule <id>] [--format <stylish|json>]
ratchet baseline init [--rule <id>] [--reason <text>]
ratchet baseline update [--rule <id>] [--reason <text>]
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
5. Tiers — `--tier` selects one tier. The number of **executed** checks in the
   selected tier must be ≥ the tier's `floor`, and must be > 0. Zero executed
   checks in the selected tier → fail ("empty selection").
6. Drift — the drift gates run as part of `check` (§7).
7. Exit code: `0` all green, `1` at least one gate failed, `2` operational error.

`paths…` restrict the run to the rules whose counter is affected by those
paths. A rule without a path scope always runs. A rule may declare
`"files": ["src/**"]` to scope it; the rule is skipped when the selected paths
do not intersect its scope.

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

### 5.6 `init`

Writes a starter `ratchet.config.jsonc` (one example rule) if no config
exists. Never overwrites an existing config.

### 5.7 `update`

See §9.

## 6. Output

### 6.1 `stylish` (default)

```
ratchet — 3 rules, 1 failed

  PASS  unwrapped-action-calls      9 ≤ 11  (call sites)
  FAIL  unexpected-inner-html       7 > 5   (files)
  FAIL  missing-baseline            no baseline for module-test-coverage

COHERENT — 2 of 3 gates passed
```

### 6.2 `json`

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
  "generated": [{ "path": "docs/MISSING_FEATURES.md", "status": "ok", "diff": null }]
}
```

Rule status values: `ok`, `over-baseline`, `under-floor`, `baseline-raised`,
`baseline-lowered`, `missing-baseline`, `counter-error`.
Generated status values: `ok`, `drift`, `new`.

## 7. Drift gates

```jsonc
{ "generated": [ { "path": "docs/MISSING_FEATURES.md", "command": ["npm", "run", "docs:gaps"], "output_arg": "--output" } ] }
```

The generator must be able to write to a **given path**:

* with `output_arg` — the binary appends the temporary output path to the argv.
* without `output_arg` — the binary snapshots the current file bytes, runs the
  command (which overwrites the file in place), reads the produced bytes, and
  **restores the original bytes** afterwards — also when the command fails.

The comparison is against the **committed** file (`git show HEAD:<path>`), not
against the working-tree copy. The diff is a unified diff (committed →
produced) with repository-relative paths.

A file that is not in `HEAD` yet has the status `new` — informational, not a
failure.

`--fix` writes the produced content to the target path and reports the gate as
fixed. `--fix` touches **only** the generated files, never hand-written text.

## 8. Git

* Repository root: `git rev-parse --show-toplevel`; fallback: the directory of
  the config file.
* Committed baseline: `git show HEAD:<baseline-path>`.
* Committed generated file: `git show HEAD:<path>`.
* No git, or not a repository → the committed comparisons are skipped, a note
  is printed, the gate still runs.

## 9. Self-update

* `ratchet update` downloads the newest release asset for the platform from the
  GitHub Releases API, verifies the SHA-256 against `checksums.txt`, replaces
  the binary atomically and prints the new version.
* An update **check** runs at most once per 24 h. The timestamp lives in
  `$XDG_CACHE_HOME/ratchet/update-check` (fallback `~/.cache/ratchet/update-check`).
  It is non-blocking, silent on failure, and prints at most one line.
* Disable with `--no-update-check` or `RATCHET_NO_UPDATE_CHECK=1`.

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
