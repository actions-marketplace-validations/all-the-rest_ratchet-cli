#!/usr/bin/env bash
#
# The update mode: the baseline moves and the change is committed.
#
# The mode is **opt-in**: a bot that moves the baseline silently on every push
# defeats the monotonicity check, so the update mode belongs to a deliberate,
# scheduled or manually triggered run.
#
# The commit is made here; the **push is not** — that is the decision of the
# calling workflow, which has to grant the permission to write.
#
# The path of the baseline is **asked**, not guessed.  The binary resolves it
# — the config's `baseline` key, the default next to the config — and the
# action asks it with `ratchet baseline path`.  A script that re-implements
# the resolution answers a different question than the binary, and a wrong
# answer is a commit of the wrong file — or a green run that committed
# nothing at all.
#
# The values are passed **attached** to their option (`--reason="…"`): a value
# that starts with a hyphen is a value and not another option.

set -euo pipefail

# The config of the project, relative to the workspace.
config="${RATCHET_CONFIG:-ratchet.config.jsonc}"

# The reason of the move.  It travels with the record, so the diff of the
# baseline file carries it — and the commit says it too.
reason="${RATCHET_REASON:-}"

# Fails the step with a message the runner shows as an error.
fail() {
  echo "::error::$1" >&2
  exit 1
}

# The reason is not optional: the baseline move is a reviewed artifact, and
# the reason is what makes it reviewable.
[ -n "$reason" ] || fail "the update mode needs a reason: the baseline move is a reviewed artifact, and the reason is what makes it reviewable (the input is 'baseline-reason')"

# The binary is the one the install step put on the PATH.
command -v ratchet >/dev/null 2>&1 || fail "ratchet is not on the PATH — the install step did not run"

# The baseline is a file of the project: the move is a commit.
git rev-parse --is-inside-work-tree >/dev/null 2>&1 ||
  fail "the update mode needs a git repository — the baseline is committed"

# The root of the repository: the paths of git and of the binary are relative
# to it.
root=$(git rev-parse --show-toplevel) || fail "the root of the repository cannot be determined"

# A commit on a detached HEAD is not reachable by the push of the workflow.
git -C "$root" symbolic-ref --quiet --short HEAD >/dev/null ||
  fail "the update mode needs a branch — a commit on a detached HEAD is not reachable by the push of the workflow"

# The directory of the temporary files of the step.
work=$(mktemp -d) || fail "a temporary directory cannot be created"
trap 'rm -rf "$work"' EXIT

# Writes the state of the working tree into the given file.
#
# The **machine-readable** form (`-z`) is used: the ordinary form quotes a
# path that contains a space, and a quoted path is not the path the binary
# named.
write_status() {
  git -C "$root" status --porcelain -z >"$1" ||
    fail "the state of the working tree cannot be read"
}

# The paths of the changed files, one per line.
#
# A record of the machine-readable form is the status (two characters), a
# space and the path.  The second field of a rename is a bare path and is
# skipped.
status_paths() {
  local record
  while IFS= read -r -d '' record; do
    # The pattern is unquoted: a quoted `?` would be a literal question mark.
    case "$record" in
      ??\ *) printf '%s\n' "${record:3}" ;;
    esac
  done <"$1"
}

# The paths of a NUL-separated list, one per line.
paths_of() {
  local path
  while IFS= read -r -d '' path; do
    printf '%s\n' "$path"
  done <"$1"
}

# The tree has to be clean: with a clean tree, the file the binary changed is
# the baseline file, and the commit contains it and nothing else.
write_status "$work/before"
dirty=$(status_paths "$work/before")
[ -z "$dirty" ] || fail "the working tree is not clean — the update mode commits the baseline and nothing else:
$dirty"

# The path of the baseline: the binary resolves it, the action asks.
baseline=$(ratchet baseline path --config="$config") ||
  fail "the path of the baseline cannot be determined — the config names it, and the binary resolves it"
[ -n "$baseline" ] || fail "the path of the baseline cannot be determined"

# The baseline moves in the declared direction only.  A count against the
# direction is not a paydown: a deliberate raise is a decision of a human.
if ! output=$(ratchet baseline update --config="$config" --reason="$reason" 2>&1); then
  # The output of the binary names the rule that refused the move.
  printf '%s\n' "$output" >&2
  fail "the baseline was not moved — a count against the declared direction is not a paydown; a deliberate raise is a decision of a human (ratchet baseline raise --rule <id> --value <n> --reason \"<text>\"). The working tree was left as the binary left it."
fi
printf '%s\n' "$output"

# With a clean tree before the move, the file the binary changed is the
# baseline file — and nothing else may have changed.
write_status "$work/after"
changed=$(status_paths "$work/after")
if [ -z "$changed" ]; then
  echo "the baseline did not move — nothing to commit"
  exit 0
fi
while IFS= read -r path; do
  [ "$path" = "$baseline" ] ||
    fail "the move changed more than the baseline: $path — nothing is committed"
done <<<"$changed"

# Only the baseline is committed: the tree of the project is not the action's.
git -C "$root" add -- "$baseline" || fail "the baseline $baseline cannot be staged"
git -C "$root" diff --cached --name-only -z >"$work/staged" ||
  fail "the staged files cannot be read"
staged=$(paths_of "$work/staged")
[ "$staged" = "$baseline" ] || fail "the commit would hold more than the baseline: $staged"

# The identity of the commit.  The action commits; it never pushes.
git_name="${RATCHET_GIT_NAME:-ratchet[bot]}"
git_email="${RATCHET_GIT_EMAIL:-ratchet[bot]@users.noreply.github.com}"

# The message names the run.
subject='ratchet: the baseline moves'
body="$reason"
if [ -n "${GITHUB_RUN_ID:-}" ]; then
  body="$body

Run: ${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-unknown}/actions/runs/$GITHUB_RUN_ID"
fi

git -C "$root" -c "user.name=$git_name" -c "user.email=$git_email" commit -m "$subject" -m "$body" -- "$baseline" ||
  fail "the baseline $baseline cannot be committed"

echo "the baseline was committed"
