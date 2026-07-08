#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_DIR="$ROOT/evidence/phase5-m5-hardening"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

mkdir -p "$EVIDENCE_DIR"

M5_SOAK_DURATION_SECONDS="${M5_SOAK_DURATION_SECONDS:-300}"
M5_SOAK_SEED="${M5_SOAK_SEED:-24069}"
M5_SOAK_FAULT_SEED="${M5_SOAK_FAULT_SEED:-1024369}"
M5_SOAK_K="${M5_SOAK_K:-64}"

if [[ "$M5_SOAK_DURATION_SECONDS" -ge 86400 ]]; then
  SOAK_OUT="$EVIDENCE_DIR/soak-24h.txt"
  LANE="24h"
else
  SOAK_OUT="$EVIDENCE_DIR/soak-smoke.txt"
  LANE="smoke"
fi

{
  echo "# M5 Soak $LANE Evidence"
  echo
  echo "captured_at_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "commit: $(git -C "$ROOT" rev-parse HEAD)"
  echo "rustc: $(rustc --version)"
  echo "duration_seconds: $M5_SOAK_DURATION_SECONDS"
  echo "seed: $M5_SOAK_SEED"
  echo "fault_seed: $M5_SOAK_FAULT_SEED"
  echo "k: $M5_SOAK_K"
  echo "fault_plan: hypervisor deterministic latency base=1 jitter=3"
  echo "tier2: not used in this lane"
  echo
  echo "## command"
  echo "\$ M5_SOAK_DURATION_SECONDS=$M5_SOAK_DURATION_SECONDS M5_SOAK_SEED=$M5_SOAK_SEED M5_SOAK_FAULT_SEED=$M5_SOAK_FAULT_SEED M5_SOAK_K=$M5_SOAK_K cargo test -p orch-server --test m5_soak -- --nocapture"
} >"$SOAK_OUT"

if M5_SOAK_DURATION_SECONDS="$M5_SOAK_DURATION_SECONDS" \
  M5_SOAK_SEED="$M5_SOAK_SEED" \
  M5_SOAK_FAULT_SEED="$M5_SOAK_FAULT_SEED" \
  M5_SOAK_K="$M5_SOAK_K" \
  cargo test -p orch-server --test m5_soak -- --nocapture >"$TMP" 2>&1
then
  grep -E '^(running [0-9]+ tests|M5_SOAK_SUMMARY|test [A-Za-z0-9_]+ \.\.\. ok|test result:)' "$TMP" >>"$SOAK_OUT" || true
else
  status=$?
  {
    echo "command failed with status $status"
    tail -n 120 "$TMP"
  } >>"$SOAK_OUT"
  exit "$status"
fi

{
  echo "# M5 Run Manifest"
  echo
  echo "updated_at_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "commit: $(git -C "$ROOT" rev-parse HEAD)"
  echo "rustc: $(rustc --version)"
  echo
  echo "| Lane | Duration seconds | K | Seed | Fault seed | Evidence |"
  echo "|---|---:|---:|---:|---:|---|"
  echo "| $LANE | $M5_SOAK_DURATION_SECONDS | $M5_SOAK_K | $M5_SOAK_SEED | $M5_SOAK_FAULT_SEED | $(basename "$SOAK_OUT") |"
  echo
  echo "Fault settings: hypervisor deterministic latency base=1 jitter=3."
  echo "Tier-2 persistence/kill hooks: not used in this lane."
} >"$EVIDENCE_DIR/run-manifest.md"

{
  echo "# M5 FAILED Reason Census"
  echo
  echo "captured_at_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "lane: $LANE"
  if grep -q 'failed_reason=none' "$SOAK_OUT"; then
    echo "observed_failed_reasons: none observed"
  else
    echo "observed_failed_reasons:"
    grep -Eo 'failed_reason=[^ ]+' "$SOAK_OUT" || true
  fi
  echo
  echo "Runbook: docs/runtime-terminal-reasons.md"
} >"$EVIDENCE_DIR/failed-reason-census.txt"

cat "$SOAK_OUT"
