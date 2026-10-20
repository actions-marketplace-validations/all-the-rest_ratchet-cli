#!/usr/bin/env bash
#
# The validate mode: the gates.
#
# The mode decides nothing but the command.  The rules, the direction and the
# tier stay in the config of the project — the action reads them, it does not
# decide them.
#
# The exit code of `ratchet check` is the exit code of the step: a failing
# gate (1) and an operational error (2) both fail the job.

set -euo pipefail

# The config of the project, relative to the workspace.
config="${RATCHET_CONFIG:-ratchet.config.jsonc}"

# The tier to run; empty means every tier.
tier="${RATCHET_TIER:-}"

# The extra arguments of the check.
args="${RATCHET_ARGS:-}"

# The values are passed **attached** to their option (`--config="…"`): a value
# that starts with a hyphen is a value and not another option.
argv=(ratchet check --config="$config")
if [ -n "$tier" ]; then
  argv+=(--tier="$tier")
fi
if [ -n "$args" ]; then
  # The extra arguments are split on whitespace; a glob is not expanded, so
  # `--ignore src/**` reaches the binary as it is written.
  set -f
  # shellcheck disable=SC2206
  extra=($args)
  set +f
  # The `+` form appends nothing when the array is empty — the old bash of
  # macOS knows no other way.
  argv+=("${extra[@]+"${extra[@]}"}")
fi

"${argv[@]}"
