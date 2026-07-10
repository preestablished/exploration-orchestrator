#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_DIR="$ROOT/evidence/phase5-m5-hardening"
TMP="$(mktemp)"
RSS_TMP="$(mktemp)"
trap 'rm -f "$TMP" "$RSS_TMP"' EXIT

mkdir -p "$EVIDENCE_DIR"

if [[ -n "$(git -C "$ROOT" status --porcelain --untracked-files=all -- . ':!evidence/phase5-m5-hardening')" ]]; then
  echo "source tree has uncommitted non-evidence changes; commit before capturing evidence" >&2
  exit 10
fi

M5_SOAK_DURATION_SECONDS="${M5_SOAK_DURATION_SECONDS:-300}"
M5_SOAK_SEED="${M5_SOAK_SEED:-24069}"
M5_SOAK_FAULT_SEED="${M5_SOAK_FAULT_SEED:-1024369}"
M5_SOAK_K="${M5_SOAK_K:-64}"
M5_SOAK_GC_EVERY_COMMITS="${M5_SOAK_GC_EVERY_COMMITS:-4}"
M5_SOAK_SCRAPE_INTERVAL_SECONDS="${M5_SOAK_SCRAPE_INTERVAL_SECONDS:-30}"
M5_SOAK_RSS_TOLERANCE_PERCENT="${M5_SOAK_RSS_TOLERANCE_PERCENT:-50}"
M5_SOAK_RSS_WARMUP_SAMPLES="${M5_SOAK_RSS_WARMUP_SAMPLES:-2}"
M5_SOAK_MIN_RSS_EVALUATED_SAMPLES="${M5_SOAK_MIN_RSS_EVALUATED_SAMPLES:-4}"

if [[ "$M5_SOAK_DURATION_SECONDS" -ge 86400 ]]; then
  SOAK_OUT="$EVIDENCE_DIR/soak-24h.txt"
  LANE="24h"
  M5_SOAK_REQUIRE_RSS_EVIDENCE="${M5_SOAK_REQUIRE_RSS_EVIDENCE:-1}"
else
  SOAK_OUT="$EVIDENCE_DIR/soak-smoke.txt"
  LANE="smoke"
  M5_SOAK_REQUIRE_RSS_EVIDENCE="${M5_SOAK_REQUIRE_RSS_EVIDENCE:-0}"
fi

if [[ "$LANE" == "24h" ]]; then
  MANIFEST_OUT="$EVIDENCE_DIR/run-manifest.md"
  CENSUS_OUT="$EVIDENCE_DIR/failed-reason-census.txt"
else
  MANIFEST_OUT="$EVIDENCE_DIR/run-manifest-$LANE.md"
  CENSUS_OUT="$EVIDENCE_DIR/failed-reason-census-$LANE.txt"
fi

rss_tree_kib() {
  local root_pid="$1"
  local pending="$root_pid"
  local pids=""
  while [[ -n "${pending// /}" ]]; do
    local next=""
    for pid in $pending; do
      pids="$pids $pid"
      next="$next $(pgrep -P "$pid" 2>/dev/null || true)"
    done
    pending="$next"
  done

  local total=0
  for pid in $pids; do
    local rss
    rss="$(ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ' || true)"
    if [[ -n "$rss" ]]; then
      total=$((total + rss))
    fi
  done
  echo "$total"
}

START_EPOCH="$(date -u +%s)"
START_UTC="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

{
  echo "# M5 Soak $LANE Evidence"
  echo
  echo "start_utc: $START_UTC"
  echo "commit: $(git -C "$ROOT" rev-parse HEAD)"
  echo "host: $(hostname)"
  echo "rustc: $(rustc --version)"
  echo "duration_seconds: $M5_SOAK_DURATION_SECONDS"
  echo "seed: $M5_SOAK_SEED"
  echo "fault_seed: $M5_SOAK_FAULT_SEED"
  echo "k: $M5_SOAK_K"
  echo "gc_every_commits: $M5_SOAK_GC_EVERY_COMMITS"
  echo "scrape_interval_seconds: $M5_SOAK_SCRAPE_INTERVAL_SECONDS"
  echo "rss_tolerance_percent: $M5_SOAK_RSS_TOLERANCE_PERCENT"
  echo "rss_warmup_samples: $M5_SOAK_RSS_WARMUP_SAMPLES"
  echo "rss_required: $M5_SOAK_REQUIRE_RSS_EVIDENCE"
  echo "rss_min_evaluated_samples: $M5_SOAK_MIN_RSS_EVALUATED_SAMPLES"
  echo "fault_plan: deterministic latency base=1 jitter=3 charged by async adapters for hypervisor/scorer/store/synth; one-shot Unavailable on hypervisor:run, scorer:score_batch, store:put_metadata, synth:propose_bursts, observatory:emit"
  echo "tier2: not used in this lane"
  echo
  echo "## command"
  echo "\$ M5_SOAK_DURATION_SECONDS=$M5_SOAK_DURATION_SECONDS M5_SOAK_SEED=$M5_SOAK_SEED M5_SOAK_FAULT_SEED=$M5_SOAK_FAULT_SEED M5_SOAK_K=$M5_SOAK_K M5_SOAK_GC_EVERY_COMMITS=$M5_SOAK_GC_EVERY_COMMITS M5_SOAK_REQUIRE_RSS_EVIDENCE=$M5_SOAK_REQUIRE_RSS_EVIDENCE M5_SOAK_MIN_RSS_EVALUATED_SAMPLES=$M5_SOAK_MIN_RSS_EVALUATED_SAMPLES cargo test -p orch-server --test m5_soak -- --nocapture"
} >"$SOAK_OUT"

M5_SOAK_DURATION_SECONDS="$M5_SOAK_DURATION_SECONDS" \
  M5_SOAK_SEED="$M5_SOAK_SEED" \
  M5_SOAK_FAULT_SEED="$M5_SOAK_FAULT_SEED" \
  M5_SOAK_K="$M5_SOAK_K" \
  M5_SOAK_GC_EVERY_COMMITS="$M5_SOAK_GC_EVERY_COMMITS" \
  cargo test -p orch-server --test m5_soak -- --nocapture >"$TMP" 2>&1 &
CMD_PID=$!

while kill -0 "$CMD_PID" 2>/dev/null; do
  echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $(rss_tree_kib "$CMD_PID")" >>"$RSS_TMP"
  sleep "$M5_SOAK_SCRAPE_INTERVAL_SECONDS" || true
done

set +e
wait "$CMD_PID"
STATUS=$?
set -e

END_EPOCH="$(date -u +%s)"
END_UTC="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
ELAPSED_SECONDS=$((END_EPOCH - START_EPOCH))

if [[ "$STATUS" -eq 0 ]]; then
  if [[ "$ELAPSED_SECONDS" -lt "$M5_SOAK_DURATION_SECONDS" ]]; then
    {
      echo "end_utc: $END_UTC"
      echo "elapsed_wall_seconds: $ELAPSED_SECONDS"
      echo "command finished before requested duration"
      tail -n 120 "$TMP"
    } >>"$SOAK_OUT"
    exit 3
  fi
  if ! grep -q '^M5_SOAK_SUMMARY ' "$TMP"; then
    {
      echo "end_utc: $END_UTC"
      echo "elapsed_wall_seconds: $ELAPSED_SECONDS"
      echo "missing M5_SOAK_SUMMARY line"
      tail -n 120 "$TMP"
    } >>"$SOAK_OUT"
    exit 4
  fi
  if ! grep -q '^M5_SOAK_FAULT_COUNTS ' "$TMP"; then
    {
      echo "end_utc: $END_UTC"
      echo "elapsed_wall_seconds: $ELAPSED_SECONDS"
      echo "missing M5_SOAK_FAULT_COUNTS line"
      tail -n 120 "$TMP"
    } >>"$SOAK_OUT"
    exit 5
  fi
  if ! grep -q '^M5_SOAK_LATENCY_CHARGED ' "$TMP"; then
    {
      echo "end_utc: $END_UTC"
      echo "elapsed_wall_seconds: $ELAPSED_SECONDS"
      echo "missing M5_SOAK_LATENCY_CHARGED line"
      tail -n 120 "$TMP"
    } >>"$SOAK_OUT"
    exit 6
  fi
  {
    echo "end_utc: $END_UTC"
    echo "elapsed_wall_seconds: $ELAPSED_SECONDS"
  } >>"$SOAK_OUT"
  grep -E '^(running [0-9]+ tests|M5_SOAK_SUMMARY|M5_SOAK_LATENCY_CHARGED|M5_SOAK_FAULT_COUNTS|test [A-Za-z0-9_]+ \.\.\. ok|test result:)' "$TMP" >>"$SOAK_OUT" || true
  if [[ -s "$RSS_TMP" ]]; then
    awk -v warmup="$M5_SOAK_RSS_WARMUP_SAMPLES" '
      {
        count += 1;
        if (count > warmup) {
          evaluated += 1;
          if (evaluated == 1 || $2 < min) min = $2;
          if ($2 > max) max = $2;
        }
      }
      END {
        omitted = count < warmup ? count : warmup;
        if (evaluated > 0) {
          pct = (min > 0) ? ((max - min) * 100.0 / min) : 0;
          printf "RSS_SUMMARY samples=%d warmup_omitted=%d evaluated_samples=%d min_kib=%d max_kib=%d delta_percent=%.2f\n", count, omitted, evaluated, min, max, pct;
        } else {
          printf "RSS_SUMMARY samples=%d warmup_omitted=%d evaluated_samples=0 min_kib=0 max_kib=0 delta_percent=0.00\n", count, omitted;
        }
      }
    ' "$RSS_TMP" >>"$SOAK_OUT"
    echo "RSS_EVIDENCE required=$M5_SOAK_REQUIRE_RSS_EVIDENCE min_evaluated_samples=$M5_SOAK_MIN_RSS_EVALUATED_SAMPLES" >>"$SOAK_OUT"
    awk -v tol="$M5_SOAK_RSS_TOLERANCE_PERCENT" -v warmup="$M5_SOAK_RSS_WARMUP_SAMPLES" -v required="$M5_SOAK_REQUIRE_RSS_EVIDENCE" -v min_eval="$M5_SOAK_MIN_RSS_EVALUATED_SAMPLES" '
      {
        count += 1;
        if (count > warmup) {
          evaluated += 1;
          if (evaluated == 1 || $2 < min) min = $2;
          if ($2 > max) max = $2;
        }
      }
      END {
        if (required == 1 && evaluated < min_eval) {
          printf "RSS evaluated samples %d below required minimum %d\n", evaluated, min_eval > "/dev/stderr";
          exit 2;
        }
        if (required == 1 && evaluated >= min_eval && min > 0) {
          pct = ((max - min) * 100.0 / min);
          if (pct > tol) {
            printf "RSS delta %.2f%% exceeds tolerance %.2f%%\n", pct, tol > "/dev/stderr";
            exit 2;
          }
        }
      }
    ' "$RSS_TMP"
  elif [[ "$M5_SOAK_REQUIRE_RSS_EVIDENCE" -eq 1 ]]; then
    echo "RSS evidence required but no RSS samples were captured" >&2
    exit 2
  fi
  echo >>"$SOAK_OUT"
else
  {
    echo "end_utc: $END_UTC"
    echo "elapsed_wall_seconds: $ELAPSED_SECONDS"
    echo "command failed with status $STATUS"
    tail -n 120 "$TMP"
  } >>"$SOAK_OUT"
  exit "$STATUS"
fi

CONFIG_HASH="$(grep -Eo 'config_hash=[0-9a-f]+' "$SOAK_OUT" | head -n1 | cut -d= -f2 || true)"

{
  echo "# M5 Run Manifest"
  echo
  echo "updated_at_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "commit: $(git -C "$ROOT" rev-parse HEAD)"
  echo "host: $(hostname)"
  echo "rustc: $(rustc --version)"
  echo "start_utc: $START_UTC"
  echo "end_utc: $END_UTC"
  echo "elapsed_wall_seconds: $ELAPSED_SECONDS"
  echo "config_hash: ${CONFIG_HASH:-unknown}"
  echo "gc_every_commits: $M5_SOAK_GC_EVERY_COMMITS"
  echo "rss_tolerance_percent: $M5_SOAK_RSS_TOLERANCE_PERCENT"
  echo "rss_warmup_samples: $M5_SOAK_RSS_WARMUP_SAMPLES"
  echo "rss_required: $M5_SOAK_REQUIRE_RSS_EVIDENCE"
  echo "rss_min_evaluated_samples: $M5_SOAK_MIN_RSS_EVALUATED_SAMPLES"
  echo
  echo "| Lane | Duration seconds | K | Seed | Fault seed | GC every commits | Evidence |"
  echo "|---|---:|---:|---:|---:|---:|---|"
  echo "| $LANE | $M5_SOAK_DURATION_SECONDS | $M5_SOAK_K | $M5_SOAK_SEED | $M5_SOAK_FAULT_SEED | $M5_SOAK_GC_EVERY_COMMITS | $(basename "$SOAK_OUT") |"
  echo
  echo "Fault settings: deterministic latency base=1 jitter=3 charged by async adapters for hypervisor/scorer/store/synth; one-shot Unavailable on hypervisor:run, scorer:score_batch, store:put_metadata, synth:propose_bursts, observatory:emit."
  echo "Fake snapshot retention: post-commit every $M5_SOAK_GC_EVERY_COMMITS commits; final retention asserts live refs equal committed refs."
  echo "Tier-2 persistence/kill hooks: not used in this lane."
} >"$MANIFEST_OUT"

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
} >"$CENSUS_OUT"

cat "$SOAK_OUT"
