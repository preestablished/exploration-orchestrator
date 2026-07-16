#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/evidence/phase5-m5-hardening/config-validation.txt"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

mkdir -p "$(dirname "$OUT")"

if [[ -n "$(git -C "$ROOT" status --porcelain --untracked-files=all -- . ':!evidence/phase5-m5-hardening')" ]]; then
  echo "source tree has uncommitted non-evidence changes; commit before capturing evidence" >&2
  exit 10
fi

{
  echo "# M5 Config Validation Evidence"
  echo
  echo "captured_at_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "commit: $(git -C "$ROOT" rev-parse HEAD)"
  echo "rustc: $(rustc --version)"
  echo "host: $(hostname)"
  echo
} >"$OUT"

run_and_capture() {
  local label="$1"
  shift

  {
    echo "## $label"
    printf '$'
    printf ' %q' "$@"
    echo
  } >>"$OUT"

  if "$@" >"$TMP" 2>&1; then
    grep -E '^(running [0-9]+ tests|test [A-Za-z0-9_]+ \.\.\. ok|test result:)' "$TMP" >>"$OUT" || true
    echo >>"$OUT"
  else
    local status=$?
    {
      echo "command failed with status $status"
      tail -n 80 "$TMP"
    } >>"$OUT"
    return "$status"
  fi
}

run_and_capture \
  "orch-core config matrix" \
  cargo test -p orch-core --test config_matrix -- --nocapture

run_and_capture \
  "orch-server config validation surface" \
  cargo test -p orch-server --test config_validation_surface -- --nocapture
