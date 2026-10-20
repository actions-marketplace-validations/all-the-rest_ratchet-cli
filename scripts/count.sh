#!/bin/sh
#
# The counters of the self-gate.
#
# The engine owns the baseline, the direction, the monotonicity and the
# report; the project owns the counting (SPEC 3.4).  This script is the
# project's half: one counter per name, printing exactly one integer on
# stdout and exiting 0.
#
# The engine runs the argv with the working directory set to the repository
# root (SPEC 3.4), so every path below is relative to that root.  The script
# only reads — it never writes into the tree.
#
# The counters are whole-file matches and one dependency walk.  Nothing here
# parses Rust: a source-level gate that has to decide which code is compiled
# — test modules, `cfg` conditions, attributes, macros — needs a Rust
# parser, and a text scan that pretends to be one produces false greens.
#
# Portable: `sh` and the tools every unix has — grep, awk, find, sort, wc
# and tr.  The constructs beyond POSIX are `grep -o` and the word boundary
# `\b` in the patterns: GNU grep documents the word boundary, BSD grep
# supports it, and both target platforms (macOS and ubuntu) have them.

set -eu

# The counters this script knows.  The unknown-counter case names them: a
# typo must not look like a counter that counts nothing.
COUNTERS="dependency-count no-machine-path-in-source no-panic-in-source no-unfinished-marker no-unsafe-in-source"

# The patterns of the source counters.  They are textual matches: the word
# in a comment or in a string counts as well, so the count can only be too
# high, never too low.
UNSAFE='\bunsafe\b'
PANIC='\b(panic!|unreachable!)'
UNFINISHED='\b(todo!|unimplemented!)'
HOME_PATH='/(Users|home)/[A-Za-z0-9._-]+'

# The Rust sources of the crate, sorted: the same tree yields the same list.
#
# `-L` follows the symbolic links: rustc compiles a symlinked module, so a
# symlinked source is part of the crate and has to be counted.  A file name
# with a newline in it splits the read loop below.  Usually the fragments do
# not exist, so the counter fails closed and names the fragment — but a name
# whose fragments all exist is read fragment by fragment, and the file itself
# is never counted.  That needs a deliberately crafted name and does not
# arise from ordinary development; it is named here rather than hidden.
#
# `find` must not fail silently: a file it cannot list is a file whose
# occurrences would be missing from the count, and a missing count is a
# false green.  A failure here ends the counter.
sources() {
  status=0
  files=$(find -L src -type f -name '*.rs' 2>/dev/null) || status=$?
  if [ "$status" -ne 0 ]; then
    printf 'count.sh: the sources under src/ cannot be listed (find exited with status %s)\n' "$status" >&2
    exit 3
  fi
  printf '%s\n' "$files" | sort
}

# Fails when the selection is empty: an empty selection is a failure, never
# a green.
require_files() {
  if [ -z "$1" ]; then
    printf 'count.sh: no Rust source found under src/ — the selection is empty\n' >&2
    exit 3
  fi
}

# The occurrences of an extended regular expression in the files on stdin.
#
# `grep` exits 1 when it matches nothing: that is a count of zero, not a
# counter error.  `grep` exits 2 when it cannot read a file: that IS a
# counter error — the counter fails with the file named instead of counting
# a silent zero.  The two are told apart by the exit status, not by
# swallowing it.
#
# The counted files are Rust sources.  A NUL byte in one of them means the
# crate does not build; BSD grep prints `Binary file <path> matches` on
# stdout for such a file and exits 0, so the counter would count that line
# as one occurrence — a false red, never a false green.
occurrences() {
  pattern=$1
  total=0
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    status=0
    matches=$(grep -oE "$pattern" "$file") || status=$?
    if [ "$status" -gt 1 ]; then
      printf 'count.sh: %s: the file cannot be read (grep exited with status %s)\n' "$file" "$status" >&2
      exit 3
    fi
    if [ -n "$matches" ]; then
      count=$(printf '%s\n' "$matches" | wc -l | tr -d '[:space:]')
      total=$((total + count))
    fi
  done
  printf '%s\n' "$total"
}

# The counters over the whole source of the crate.
count_source() {
  pattern=$1
  files=$(sources)
  require_files "$files"
  # The status of the pipeline is the status of `occurrences`: a counter
  # error fails the counter, it never counts a silent zero.
  printf '%s\n' "$files" | occurrences "$pattern"
}

# The dependency entries of the manifest.
#
# Counted are the entries of every dependency section of this package —
# `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]` and their
# target-specific forms — in the inline form (`name = "1"`) and in the table
# form (`[dependencies.name]`).  The other sections are not dependencies of
# this package: `[workspace.dependencies]` in particular is inherited by the
# members of a workspace, not depended on by the root.
#
# A minimal TOML walk: the current section header decides whether a line is
# an entry.  An entry is a key at the start of a line; the continuation line
# of a multi-line inline table is indented and is not an entry.
#
# Two assumptions, both over-counting — the safe direction: a multi-line
# inline table whose continuation starts at column 0 is counted twice, and a
# manifest with CRLF line endings counts its blank lines as entries.
DEPENDENCY_AWK='
# Splits a section header into its dot-separated segments.  A quoted segment
# keeps its dots: target.<cfg>.dependencies has three segments.
function split_header(text,   i, c, n, current, quoted) {
  n = 0
  current = ""
  quoted = ""
  for (i = 1; i <= length(text); i++) {
    c = substr(text, i, 1)
    if (quoted != "") {
      if (c == quoted) { quoted = "" }
      current = current c
      continue
    }
    if (c == "\"" || c == quote_char) { quoted = c; current = current c; continue }
    if (c == ".") { n++; segment[n] = current; current = ""; continue }
    current = current c
  }
  n++
  segment[n] = current
  return n
}

BEGIN { mode = "other"; count = 0; quote_char = sprintf("%c", 39) }

/^\[/ {
  header = $0
  sub(/^\[+/, "", header)
  sub(/\]+[[:space:]]*$/, "", header)
  n = split_header(header)
  kind_index = 0
  for (i = n; i >= 1; i--) {
    if (segment[i] == "dependencies" || segment[i] == "dev-dependencies" || segment[i] == "build-dependencies") {
      kind_index = i
      break
    }
  }
  mode = "other"
  if (kind_index == 1 || (kind_index == 3 && segment[1] == "target")) {
    if (kind_index == n) {
      mode = "entries"
    } else if (kind_index == n - 1) {
      mode = "table"
      count++
    } else {
      mode = "fields"
    }
  }
  next
}

mode == "entries" {
  if ($0 !~ /^[^ \t]/) { next }
  if ($0 ~ /^#/) { next }
  count++
}

END { print count }
'

# The dependency entries of the manifest.
count_dependencies() {
  awk "$DEPENDENCY_AWK" Cargo.toml
}

main() {
  if [ "$#" -ne 1 ]; then
    printf 'count.sh: usage: count.sh <counter>\n' >&2
    printf 'count.sh: the accepted counters are: %s\n' "$COUNTERS" >&2
    exit 2
  fi

  case "$1" in
    dependency-count)
      count_dependencies
      ;;
    no-unsafe-in-source)
      count_source "$UNSAFE"
      ;;
    no-panic-in-source)
      count_source "$PANIC"
      ;;
    no-unfinished-marker)
      count_source "$UNFINISHED"
      ;;
    no-machine-path-in-source)
      count_source "$HOME_PATH"
      ;;
    *)
      printf 'count.sh: unknown counter: %s\n' "$1" >&2
      printf 'count.sh: the accepted counters are: %s\n' "$COUNTERS" >&2
      exit 2
      ;;
  esac
}

main "$@"
