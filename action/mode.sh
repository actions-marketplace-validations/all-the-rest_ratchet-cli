#!/usr/bin/env bash
#
# The mode check: an unknown mode fails the step.
#
# The check is a **script** and not an `if:` on the step: a composite step
# without an `if:` always runs, so the check cannot be switched off by an
# expression that no test covers.  A mode that runs neither of the two mode
# steps would leave the job green without having done anything.
#
# The mode comes from the **environment**, never from an interpolation into
# the command: an input that reaches a shell as text can become a command.

set -euo pipefail

mode="${RATCHET_MODE:-}"

case "$mode" in
  validate | update) exit 0 ;;
esac

# The message names the mode.  A newline in the mode would make a second
# workflow command out of the message, so it is flattened first.
flat=$(printf '%s' "$mode" | tr '\n\r' '  ')
echo "::error::unknown mode '$flat' — the modes are 'validate' and 'update'" >&2
exit 1
