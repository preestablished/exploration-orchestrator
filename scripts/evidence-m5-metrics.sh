#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/evidence/phase5-m5-hardening/metrics-diff.txt"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

mkdir -p "$(dirname "$OUT")"

{
  echo "# M5 Metrics Evidence"
  echo
  echo "captured_at_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "commit: $(git -C "$ROOT" rev-parse HEAD)"
  echo "rustc: $(rustc --version)"
  echo
  echo "## Before metric families"
  echo
  echo "- orchestratord_up"
  echo
  echo "## After metric families"
  echo
  for family in \
    orch_archive_cells \
    orch_batch_latency_seconds \
    orch_best_score \
    orch_escalation_level \
    orch_expansions_total \
    orch_frontier_size \
    orch_jobs_failed_total \
    orch_nodes_total \
    orch_observatory_dropped_total \
    orch_pipeline_queue_depth \
    orch_slot_utilization
  do
    echo "- $family"
  done
  echo
  echo "## Required label values"
  echo
  echo "- orch_nodes_total verdict: kept, dup, regression"
  echo "- orch_pipeline_queue_depth stage: submit, complete"
  echo "- orch_batch_latency_seconds stage: select, execute, commit"
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
  "orch-server metrics surface" \
  cargo test -p orch-server --test metrics_surface -- --nocapture

run_and_capture \
  "orchestratord HTTP metrics endpoint" \
  cargo test -p orchestratord --test http_metrics -- --nocapture
