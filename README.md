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
ratchet check --tier fast # run the gates of one tier
ratchet count           # print the current count per rule
ratchet baseline init   # write the current count as the baseline
ratchet baseline update # pay a ceiling down / a floor up — in that direction only
ratchet drift           # run the drift gates: the generated files
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

### The drift gates

A **generated file** is a file that a generator produces. The `generated`
entries of the config name them:

```jsonc
"generated": [
  { "path": "docs/MISSING_FEATURES.md", "command": ["npm", "run", "docs:gaps"], "output_arg": "--output" }
]
```

The gate compares the **committed** file (`git show HEAD:<path>`) with what
the generator produces: a stale committed file is the defect, and the gate
fails with the unified diff of the difference:

```
--- docs/MISSING_FEATURES.md
+++ docs/MISSING_FEATURES.md
@@ -1,3 +1,3 @@
 # Title
 
-stale content
+fresh content
```

The diff is a **patch**: `patch -p0` applies it to the committed file. The
newline at the end of a file is part of the file, so a file that ends with a
newline and one that does not differ — and the difference shows up in the
diff. A generated file that is not valid UTF-8 produces a **lossy** diff:
the bytes that are not text are shown as the replacement character. The
verdict is still right — it is computed on the raw bytes — but the human
artifact is not a faithful rendering of a binary file.

The status of a generated file is `ok` (in sync), `drift` (the committed file
differs — a failed gate) or `new` (not committed yet — informational, not a
failure). A generator that cannot be run — or that runs and produces
nothing — is an **operational error** (exit 2), not a failed gate. So is a
committed file that cannot be read: a broken reference must not disable a
gate, and a committed path that is a directory is not a file at all. A
repository whose `HEAD` points at a branch that has no commit yet has nothing
committed yet (`new`); a repository whose `HEAD` names a commit that cannot be
read is damaged, and that is an operational error as well.

A `generated[]` entry may declare a `tier` (`fast` | `slow`, default `fast`).
`--tier fast` skips the slow generators: a generator that needs a database, a
browser or the network belongs to the slow tier, and a fast tier that waits
for it is a fast tier that is not fast.

Two generator modes exist:

- **with `output_arg`** — the generator writes to a temporary path; the tree
  is never touched.
- **without `output_arg`** — the generator writes the file **in place**. The
  binary then owns a **verified restore**: it snapshots the original bytes,
  the file mode *and* the time the file was written last, runs the
  generator, reads the produced bytes, writes the snapshot to a temporary
  file in the same directory, gives it the original mode and time and
  **renames** it over the target — a rename replaces a symbolic link instead
  of writing through it — verifies the restore and removes a directory the
  generator created and left empty. If the target **is** a symbolic link, the
  restore writes the file **behind** the link and recreates the link: a
  rename over the link would replace it with a regular file and change the
  tree, and the tree must not change. A restore that fails ends the run with
  an operational error that names the file and says that the working tree was
  modified: a swallowed restore error is a mutated tree that nobody was told
  about.

  Before the generator runs, the file is touched with an **old** time — an
  hour into the past. The engine can then tell whether the generator wrote
  the file — a generator that leaves it alone produced nothing, and the
  comparison of the committed file with itself would be a green that says
  nothing. The marker works on every filesystem, whatever its timestamp
  granularity is.

  The generator sees an input that is an hour older than it is. A generator
  that compares the time of its input with the time of its output therefore
  regenerates instead of skipping — and a generator that embeds the time of
  its input in its output would produce a permanent drift, which the
  determinism rule forbids anyway. The tree is restored exactly afterwards:
  the bytes, the mode **and** the time.

  What the engine can see is the **inode**, the **time** and the **bytes** of
  the file. A write that changes none of them — the same bytes, written over
  the same inode, with the time of the original restored — is reported as
  *"produced nothing"*. That is the fail-closed direction: a false red,
  never a false green. A generator that **replaces** the file (`cp -p` and
  `mv`, the reproducible-build pattern) leaves a new inode behind and is
  recognized as a write ✓.

`ratchet drift` runs the drift gates alone; `ratchet check` runs them as part
of the whole run. A drift command without a generated entry fails: an empty
selection is never a pass. `--fix` writes the produced content into the
generated files — and only into the generated files, never into hand-written
text. The gate stays **failed**: the committed file is still stale, because
the gate measures the **committed** artifact. It turns green once the fix is
committed. A run with `--fix` in a pipeline is therefore never green against a
stale HEAD; that is deliberate, not a bug.

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
| `ratchet drift` | Runs the drift gates — the generated files — and nothing else. `--path` restricts the run to one generated file. |
| `ratchet init` | Writes a starter `ratchet.config.jsonc` if no config exists. An existing config — also one in a parent directory of the repository — is never overwritten. |

### Options

| Option | Meaning |
|---|---|
| `--config <path>` | Use this config file. |
| `--no-config` | Do not look for a config; only valid together with `--config`. |
| `--format <stylish\|json>` | The output format, `stylish` by default. |
| `--rule <id>` | Restrict the run to one rule. |
| `--fix` | Apply the fixable: write the produced content into the generated files. The gate stays failed until the fix is committed. |
| `--tier <fast\|slow>` | Run only the rules of that tier — and only the generators of that tier. A `check` option: `ratchet drift` runs every tier. |
| `--ignore <pattern>` | Exclude the paths that match the pattern from the **rules' path scope** (repeatable). It does not exclude a drift gate. |
| `--debug` | Print the resolved config and every counter invocation (on stderr). |
| `--quiet` | Only print failures. |
| `--version`, `--help` | |

`baseline init` and `baseline update` accept `--reason <text>`, which is stored
in the baseline record.

### The tiers

The config names the tiers and their **floor** — the minimum number of checks
that must *execute* in that tier:

```jsonc
"tiers": { "fast": { "floor": 5 }, "slow": { "floor": 1 } }
```

`--tier fast` runs the rules of the fast tier only; without it, every rule
runs. The floor of **every tier that runs** is enforced: with `--tier` that is
the selected tier, without it every tier of the config. A tier that executes
fewer checks than its floor fails, and a tier that executes **no** check fails
as well — an empty selection is never a pass. A check that ran and failed
counts as executed (it fails anyway); a check that was never selected does
not.

The floor is what stops a tier from being quietly narrowed — a narrowing that
only shows up when somebody selects the tier by hand is a narrowing that
survives.

The floor counts the **checks** that execute in the tier, and a check is a
rule **or a drift gate**: a rule that ran and failed counts as executed, and
so does a generator that ran. A tier that declares a floor therefore needs
that many rules and generators — a tier whose promised checks are all missing
is exactly the narrowing the floor exists to catch.

The drift gates are tiered like the rules: a `generated[]` entry belongs to
the `fast` tier unless it says otherwise.

### The path exclusion

`--ignore` takes glob patterns of paths to exclude from the selection. A rule
whose scope does not intersect the remaining paths is skipped. Ignoring a
path never turns a failing gate green: a rule that would have run on the
ignored path still runs on the rest.

`--ignore` applies to the **rules' path scope**. It does not exclude a drift
gate: a generated file is registered deliberately, and a pattern that happens
to match it must not silently disable the gate.

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
  "generated": [],
  "notes": []
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

The `generated` array carries one entry per generated file: its `path`, its
`status` (`ok`, `drift` or `new`) and the unified `diff` — absent when there
is none. `summary.generated` counts the generated files, `summary.drifted`
the ones that drifted. `summary.executed` counts the **checks** that ran: the
rules and the drift gates.

```
{
  "path": "docs/MISSING_FEATURES.md",
  "status": "drift",
  "diff": "--- docs/MISSING_FEATURES.md\n+++ docs/MISSING_FEATURES.md\n@@ -1,3 +1,3 @@\n # Title\n \n-stale content\n+fresh content\n",
  "detail": "differs from the committed file"
}
```

The `notes` array carries the same notes the human output prints on stderr —
the tier floor, the skipped monotonicity comparison, a skipped drift gate. It
is empty when there is nothing to say. A machine consumer must be able to
learn why the run is not green **without** the note being only on stderr: a
report that says `ok: false` and gives no reason is a report that cannot be
acted on.

```
"notes": ["the tier `fast` executed 2 checks, the floor is 5"]
```

A baseline that moved against its committed counterpart is reported as
`baseline-raised` / `baseline-lowered` — even when the count comparison failed
as well: the baseline itself is the invalid artifact, and the count comparison
is reported as an additional note. A run that executes no **check** at all
fails as well: an empty selection is never a pass. A check is a rule **or a
drift gate**, so a project whose only gates are drift gates is green. A
`count` over a config without rules is a successful run.

Every path in the output is relative to the repository root: an absolute path,
a machine name or a timestamp would make the same tree produce different bytes.

The `count`, `baseline` and `init` commands use the same envelope
(`schema`, `ok`, `summary`, `rules`, `generated`, `notes`); `count` reports the count
without a baseline (`"baseline": null`), and the `baseline` commands report one
entry per rule with an `action` (`added`, `kept`, `updated`, `refused`).

## The exit codes

| Code | Meaning |
|---|---|
| `0` | green |
| `1` | at least one gate failed |
| `2` | operational error: invalid config, missing config, crashed counter, unreadable file, unknown command or path, a committed baseline that does not parse |

## This repo gates itself

The repository is gated with the binary it ships. `ratchet.config.jsonc` holds
the rules, `scripts/count.sh` holds the counters, `ratchet.baseline.json` holds
the baselines — the baseline file is written by the tool, never by hand.

| Rule | Class | Unit | Direction | Value today |
|---|---|---|---|---|
| `dependency-count` | supply-chain debt | dependency entries | ceiling | 6 |
| `no-machine-path-in-source` | a machine-specific path baked into the source | occurrences | ceiling | 0 |
| `no-panic-in-source` | crash instead of a clean failure | occurrences | ceiling | 0 |
| `no-unfinished-marker` | unfinished work | occurrences | ceiling | 0 |
| `no-unsafe-in-source` | unsafe code | occurrences | ceiling | 0 |

Four of the five rules count zero today: they are **boolean gates** — *no
occurrence may appear* — and their baseline must never move. `dependency-count`
is the only ratchet: its baseline of 6 dependency entries may only decrease,
and `ratchet baseline update` is the only way to move it.

The counters live in `scripts/count.sh`, one case per counter name; each one
prints exactly one integer. They are plain `sh` and the tools every unix has
— grep, awk, find, sort, wc and tr. The constructs beyond POSIX are `grep -o`
and the word boundary `\b` in the patterns: GNU grep documents the word
boundary, BSD grep supports it, and both target platforms (macOS and ubuntu)
have them.

The counters rest on three assumptions, stated here so that the class and the
count describe the same thing:

- The four source counters match every `*.rs` file under `src/` — the crate's
  own sources, the inline `#[cfg(test)]` modules included. The integration
  tests under `tests/` are a separate crate and are **not** counted. The match
  is **textual**: the word in a comment or in a string counts as well, so the
  count can only be too high, never too low.
- The counted files are Rust sources. A NUL byte in one of them means the
  crate does not build; BSD grep prints `Binary file <path> matches` on stdout
  for such a file, so the counter would count that line as one occurrence — a
  false red, never a false green.
- The dependency counter counts **entries**: a key at the start of a line. A
  multi-line inline table whose continuation starts at column 0 is counted
  twice, and a manifest with CRLF line endings counts its blank lines as
  entries. Both over-count, which is the safe direction.

Locally, after a build:

```sh
cargo build
./target/debug/ratchet check
```

In CI the same gate runs on both matrix legs (ubuntu and macos) as the step
*Self-gate: ratchet check*.

What the gates do **not** cover, named here rather than in a footnote:

- **The drift between `SPEC.md`, this README and `--help`.** The test suite
  checks the documented **stylish report example** in this README and in
  `SPEC.md` byte for byte; `--help` is covered by neither the gate nor the
  tests.
- **The engine writes to the terminal.** A source-level gate for it has to
  decide which code is *compiled* — test modules, `cfg` conditions,
  attributes, macros — and that is a Rust parser, not a text scan. A scan
  produces false greens: a multi-line attribute that rustfmt keeps
  multi-line —

  ```rust
  #[cfg(any(
      feature = "alpha",
      feature = "beta"
  ))]
  ```

  — swallows the rest of the file, and a `println!` behind it counts zero:
  the gate reports green. It also produces false reds: a `#[test] fn` at file
  scope breaks a scanner that looks for a `mod` behind the attribute, and the
  counter fails on ordinary Rust. A gate that can be silently defeated is
  worse than no gate, so the class is not gated. What covers it instead: the
  test `the_stylish_output_is_the_documented_one` compares the **whole
  stdout** of `ratchet check` byte for byte with the documented example, so a
  stray print that reaches the report path fails the test — and CI runs the
  test on every push to `main` and on every pull request targeting `main`. A
  print in a path that no test exercises is not caught.
- **A panic that is not a `panic!`.** The `no-panic-in-source` gate counts
  `panic!` and `unreachable!` only. `unwrap()`, `expect()` and `assert!` are
  not counted — 65 such occurrences exist in `src/`, all of them inside the
  `#[cfg(test)]` modules, while the gate reports 0. A ceiling on them would
  turn every new test red, so the class stays open and is named here.
- **The integration tests.** The four source counters match every `*.rs` file
  under `src/`; the integration tests under `tests/` are a separate crate and
  stay outside the count. A panic in a test is how a test fails, so the class
  *crash instead of a clean failure* does not apply to them — the class is
  about the tool's runtime behaviour. The price is named: the `panic!` in
  `tests/cli.rs` is not counted.

## Not implemented yet

`SPEC.md` describes more than this binary does today. The self-update
(`ratchet update`) is specified but not implemented, and the release workflow
does not exist either: there is no GitHub Release to download yet, build the
binary from source.

## Specification

`SPEC.md` is the normative specification. Where the code and the spec disagree,
the spec wins and the code is the bug.
