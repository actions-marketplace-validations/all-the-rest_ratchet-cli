# ratchet

`ratchet` is a standalone binary that enforces **ratchet gates**: quality
gates that compare a **count** against a **baseline** that may only move in one
declared direction.

## The idea in five lines

1. Every rule counts one defect class — layer violations, missing tests, raw
   HTML sinks.
2. The count is compared against a **baseline**, a number in the repository.
3. A **ceiling** rule fails when the count rises above its baseline; a
   **floor** rule fails when the count falls below it.
4. The baseline itself may only move in the declared direction: a ceiling may
   only decrease, a floor may only increase. Raising a ceiling to make a gate
   green is a **failed gate**, not a fix.
5. The baseline moves only when a human says so — `ratchet baseline update`,
   never automatically.

The engine owns the baseline, the direction, the monotonicity and the
reporting. The project owns the counting: a rule produces its number through a
command or a JSON file.

## Install

There is no release yet — the release workflow is part of a later step. Until
one exists, build the binary from source and put it on your `PATH`:

```sh
cargo build --release
install -m 0755 target/release/ratchet /usr/local/bin/
ratchet --version
```

When a release is published, it will consist of per-OS/arch tarballs with a
`checksums.txt`; installing it will look like this:

```sh
tar -xzf ratchet-<version>-<os>-<arch>.tar.gz
sha256sum --check checksums.txt
mv ratchet /usr/local/bin/
```

The intended way to update an installed binary is `ratchet update`: it
downloads the newest release asset for the platform, verifies the SHA-256
against `checksums.txt` and replaces the binary. The self-update is not
implemented in this build yet — until it is, replace the binary by hand.

## Usage

```sh
ratchet                 # the same as `ratchet check`
ratchet check           # run every gate
ratchet check src/      # run the gates whose scope touches src/
ratchet count           # print the current count per rule
ratchet baseline init   # write the current count as the baseline
ratchet baseline update # pay a ceiling down / a floor up — in that direction only
ratchet init            # write a starter config
```

## Configuration

The config is the only source of truth for the rules. It is a *flat* config in
the ESLint shape: an array of config objects that are merged. A later object
overrides an earlier one for the same rule id. A single config object at the
root is accepted as shorthand for a one-element array.

The baseline file lives next to the config and must lie **inside the
repository root**: a baseline that cannot be compared with its committed
version is a gate that does not gate. A config that points outside the root is
a configuration error.

The file is `ratchet.config.jsonc` (JSON with comments and trailing commas);
`ratchet.config.json`, `ratchet.config.mjs` and `ratchet.config.cjs` are
accepted as well. The lookup walks up from the working directory to the git
root.

```jsonc
[
  {
    // Optional label for this config object.
    "name": "noema",

    // Per tier settings; the minimum number of checks that must execute.
    "tiers": {
      "fast": { "floor": 5 },
      "slow": { "floor": 1 }
    },

    // One rule per defect class.
    "rules": {
      "unwrapped-action-calls": {
        // The defect class the rule counts.
        "class": "layer violation",
        // The incident that motivated the rule.  For a class that has already
        // occurred, the incident names that concrete instance; for a
        // prevention rule — a class whose count is zero today — it says so
        // plainly ("prevention: no instance in this project yet").
        "incident": "F-09: a server action was called without the shared error helper",
        // What one count unit is.
        "unit": "call sites",
        // The allowed direction of the baseline: "ceiling" or "floor".
        "direction": "ceiling",
        // Which tier the rule belongs to; "fast" (default) or "slow".
        "tier": "fast",
        // How the count is produced: a command (no shell) or a JSON file.
        "count": { "command": ["node", "scripts/audit-inventory.mjs", "--print"] }
      },

      "unexpected-inner-html": {
        "class": "layer violation",
        "incident": "an unescaped HTML sink reached the renderer",
        "unit": "files",
        "direction": "ceiling",
        "tier": "fast",
        // A JSON file, optionally with a dotted path into it.
        "count": { "file": "docs/_audit-raw.json", "path": "coherence.dangerouslySetInnerHTML" },
        // Glob patterns removed from the counted items; the items are
        // repository-relative paths.
        "exclude": ["src/lib/markdown.ts", "src/lib/html-sanitize.ts"],
        // The scope of the rule; without it the rule always runs.
        "files": ["src/**"]
      }
    }
  }
]
```

An unknown field anywhere — in a config object, in a rule or in a counter — is
a **configuration error** (exit code 2) that names the offending path, e.g.
`rules.unwrapped-action-calls.direction`. Typos fail loudly instead of
silently disabling a gate. An empty `class`, `incident` or `unit` is a
configuration error as well: an empty string is a missing value that happens
to pass the type check.

### The incident

The `incident` is the incident that **motivated** the rule. For a rule whose
class has already occurred, it names that concrete instance — it is what a
later reader judges the rule against when the count moves:

```jsonc
"incident": "F-09: a server action was called without the shared error helper"
```

For a **prevention rule** — a class whose count is zero today — the incident
says so plainly instead of naming an incident that did not happen:

```jsonc
"incident": "prevention: no instance in this project yet"
```

A gate that claims a history it does not have is worse than no gate: the next
reader cannot tell a real regression from a remembered one.

### The counter

A counter produces one of these values:

| Value | Meaning |
|---|---|
| integer | the count |
| array of strings | the items; the count is the number of items after `exclude` |
| `{"count": N}` | the count |
| `{"count": N, "items": [...]}` | the count and the items |

For a command counter the stdout is parsed: trimmed, then tried as an integer,
then as JSON in the shapes above. The command runs in the repository root,
without a shell — the arguments are passed through verbatim. A counter that
fails, produces garbage or a negative number is a counter error.

`exclude` needs a counter that yields items; a scalar counter plus `exclude` is
a configuration error.

### The globs

`exclude` and `files` are globs over repository-relative paths: `*` matches any
run of characters except `/`, `**` matches zero or more path segments, `?` and
`[...]` work as usual. A pattern that ends in `/**` also matches the directory
itself — `src/**` matches `src` and everything below it. A scope that selects
nothing is a failure, not a pass: the rule that would have run is not silently
skipped.

### A generated artifact

A file counter can regenerate its artifact before the count is read — a count
read from a stale artifact is a gate that lies:

```jsonc
{
  "file": "docs/_audit-raw.json",
  "path": "coherence.unwrappedCallSites",
  "generate": ["node", "scripts/audit-inventory.mjs"],
  "output_arg": "--output"
}
```

The binary appends a **temporary output path** to the argv; the generator
writes there and the count is read from the fresh artifact. `output_arg` is
mandatory: a generator that cannot write to a given path would have to write
into the tree, and the tree is not the engine's to write. `generate` without
`output_arg` is a configuration error, and so is `output_arg` without
`generate`. The working tree stays exactly as it was.

### The JavaScript config

`ratchet.config.mjs` / `ratchet.config.cjs` exports the same JSON shape as its
default export and is evaluated with `node`. The config is **data only** — a
rule counts through `command` or `file`, never through a function.

```js
export default [
  {
    name: "noema",
    rules: {
      "unwrapped-action-calls": {
        class: "layer violation",
        incident: "F-09: a server action was called without the shared error helper",
        unit: "call sites",
        direction: "ceiling",
        count: { command: ["node", "scripts/audit-inventory.mjs", "--print"] },
      },
    },
  },
];
```

Known limitation: a repository whose path contains a **backslash** cannot be
loaded through the JS bridge — node rejects the percent-encoded backslash in a
file URL. The failure is closed and the error names the cause; use
`ratchet.config.jsonc` in such a checkout.

## The baseline file

`ratchet.baseline.json` is written by the tool and never hand-edited. The
rules are sorted by id, so the file is a stable diff.

```json
{
  "version": 1,
  "rules": {
    "unwrapped-action-calls": { "value": 11, "reason": "F-09 remainder, tracked in P1.2" }
  }
}
```

## The commands

| Command | What it does |
|---|---|
| `ratchet [check] [paths…]` | Runs every gate and compares the count with the baseline. The default command. `paths…` restricts the run to the rules whose `files` scope touches them; a path that does not exist is an operational error. |
| `ratchet count` | Prints the current count per rule, without a comparison. |
| `ratchet baseline init` | Writes the current count as the baseline of every rule that has none. Existing baselines are never touched. |
| `ratchet baseline update` | Moves the baseline towards the current count — **only** in the allowed direction. A ceiling may only decrease, a floor may only increase. A rule that may not move is reported as refused and left alone. |
| `ratchet init` | Writes a starter `ratchet.config.jsonc` if no config exists. An existing config — also one in a parent directory of the repository — is never overwritten. |

### Options

| Option | Meaning |
|---|---|
| `--config <path>` | Use this config file. |
| `--no-config` | Do not look for a config; only valid together with `--config`. |
| `--format <stylish\|json>` | The output format, `stylish` by default. |
| `--rule <id>` | Restrict the run to one rule. |
| `--debug` | Print the resolved config and every counter invocation (on stderr). |
| `--quiet` | Only print failures. |
| `--version`, `--help` | |

`baseline init` and `baseline update` accept `--reason <text>`, which is stored
in the baseline record.

## The output

`stylish` (the default):

```
ratchet — 3 rules, 2 failed

  FAIL  missing-baseline              no baseline for module-test-coverage
  FAIL  unexpected-inner-html         7 > 5  (files)
  PASS  unwrapped-action-calls        9 ≤ 11  (call sites)

NOT COHERENT — 2 of 3 gates failed
```

### The report and the diagnostic channel

The **report** — the stylish output and the JSON document — contains **only
ratchet's own words**: a rule id, a status, a count, a baseline, a path
relative to the repository root, the exit status of a failed subprocess. The
output of a counter, a generator or `node` is never relayed into the report: a
subprocess that prints a timestamp, a hostname or an absolute path would make
the same tree produce different bytes.

A failed subprocess is therefore reported as *what* it was, not as *what it
printed*:

```
  FAIL  a        the counter exited with status 1
```

The raw output belongs to the **diagnostic channel**: `--debug` prints the
resolved config, every counter invocation and the raw output of every
subprocess. `--debug` is a transcript for one human reading one run and is
exempt from the determinism rule — two runs with `--debug` may differ.

`json` prints nothing but the JSON object on stdout; the notes and the debug
log go to stderr:

```json
{
  "schema": 1,
  "ok": false,
  "summary": { "rules": 3, "failed": 1, "executed": 3, "generated": 0, "drifted": 0 },
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
  "generated": []
}
```

The status of a rule is one of `ok`, `over-baseline`, `under-floor`,
`baseline-raised`, `baseline-lowered`, `missing-baseline` and `counter-error`.
A rule entry may carry a `detail` field: the reason for a status that is not
`ok`. It is absent when there is nothing to say, so a machine consumer can
learn **why** a rule failed without parsing the human output.

```
{
  "id": "unexpected-inner-html",
  "status": "over-baseline",
  "count": 7,
  "baseline": 5,
  "direction": "ceiling",
  "unit": "files",
  "class": "layer violation",
  "tier": "fast",
  "detail": "count 7 > baseline 5"
}
```

A baseline that moved against its committed counterpart is reported as
`baseline-raised` / `baseline-lowered` — even when the count comparison failed
as well: the baseline itself is the invalid artifact, and the count comparison
is reported as an additional note. A run that executes no rule at all fails as
well: an empty selection is never a pass. A `count` over a config without
rules is a successful run.

Every path in the output is relative to the repository root: an absolute path,
a machine name or a timestamp would make the same tree produce different bytes.

The `count`, `baseline` and `init` commands use the same envelope
(`schema`, `ok`, `summary`, `rules`, `generated`); `count` reports the count
without a baseline (`"baseline": null`), and the `baseline` commands report one
entry per rule with an `action` (`added`, `kept`, `updated`, `refused`).

## The exit codes

| Code | Meaning |
|---|---|
| `0` | green |
| `1` | at least one gate failed |
| `2` | operational error: invalid config, missing config, crashed counter, unreadable file, unknown command or path, a committed baseline that does not parse |

## Not implemented yet

`SPEC.md` describes more than this binary does today. The drift gates
(`ratchet drift`, `--fix`), the tier floors (`--tier`), the path exclusion
(`--ignore`) and the self-update (`ratchet update`) are specified but not
implemented — a config may already contain `tiers` and `generated` entries,
they are simply not evaluated yet. The release workflow does not exist either:
there is no GitHub Release to download yet, build the binary from source.

## Specification

`SPEC.md` is the normative specification. Where the code and the spec disagree,
the spec wins and the code is the bug.
