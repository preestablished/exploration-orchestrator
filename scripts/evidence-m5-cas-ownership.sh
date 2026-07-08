#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/evidence/phase5-m5-hardening/cas-ownership.txt"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

mkdir -p "$(dirname "$OUT")"

{
  echo "# M5 CAS Ownership Evidence"
  echo
  echo "captured_at_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "commit: $(git -C "$ROOT" rev-parse HEAD)"
  echo "rustc: $(rustc --version)"
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
  "orch-server CAS ownership loss windows" \
  cargo test -p orch-server --test cas_ownership_loss -- --nocapture

run_and_capture \
  "runtime terminal reason runbook drift" \
  cargo test -p orch-core --test runtime_reasons -- --nocapture
