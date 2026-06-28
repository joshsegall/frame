#!/usr/bin/env bash
#
# Warn if the REAL global frame registry (~/.config/frame/projects.toml) contains
# stale entries whose project directory no longer exists. That almost always
# means a manual smoke test leaked into your actual project list -- run the
# binary via scripts/fr-dev to avoid it.
#
# This guard only WARNS; it never mutates the registry and never blocks a commit
# (mutating shared global state as a side effect of `git commit` would be
# surprising). To clean up, run `fr projects prune`.
#
# Used by .githooks/pre-commit, but also runnable on its own:
#   scripts/check-registry.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fr_bin="$repo_root/target/debug/fr"

# Nothing built yet -> nothing to check. Stay silent so we never block commits.
[[ -x "$fr_bin" ]] || exit 0

# Inspect the REAL registry (deliberately NOT sandboxed like scripts/fr-dev).
report="$("$fr_bin" projects prune --dry-run 2>/dev/null || true)"

if printf '%s' "$report" | grep -q '^Would remove'; then
  echo "warning: your global frame registry has stale (not-found) entries:" >&2
  printf '%s\n' "$report" | sed 's/^/  /' >&2
  echo "  -> clean up with: fr projects prune" >&2
  echo "  -> avoid this by smoke-testing via: scripts/fr-dev <args>" >&2
fi

exit 0
