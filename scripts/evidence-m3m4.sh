#!/usr/bin/env bash
# Evidence pass for the M3+M4 acceptance suites (plan 04-verification.md).
# Runs the named suites with pinned seeds and tees trimmed summaries into
# evidence/phase5-m3-m4/. CHAOS_SEEDS_PER_POINT widens the Tier-1 lattice
# (CI default is 2; the evidence pass uses 5).
set -euo pipefail
cd "$(dirname "$0")/.."
out=evidence/phase5-m3-m4
mkdir -p "$out"

{
  echo "# Evidence run manifest"
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "host: $(uname -a)"
  echo "toolchain: $(rustc --version)"
  echo "commit: $(git rev-parse HEAD)"
  echo "chaos seeds per point: ${CHAOS_SEEDS_PER_POINT:-5}"
} > "$out/run-manifest.md"

run() {
  local name="$1"; shift
  echo "== $name =="
  "$@" 2>&1 | grep -E "^test |test result|seed-gate:|fast-replay:" || true
}

{
  run "M3 utilization"        cargo test -p orch-sched --test utilization
  run "M3 backpressure"       cargo test -p orch-sched --test backpressure
  run "M3 retry equivalence"  cargo test -p orch-sched --test retry_equivalence
  run "M3 shrink/grow"        cargo test -p orch-sched --test shrink_grow
  run "M3 expansion context"  cargo test -p orch-sched --test expansion_context
} | tee "$out/m3-acceptance.txt"

{
  run "M4 autonomy (10 seeds)" cargo test -p orch-server --test autonomy
  run "M4 plateau ladder"      cargo test -p orch-server --test plateau_ladder
  run "M4 fast replay"         cargo test -p orch-server --test fast_replay
  run "M4 pause/resume"        cargo test -p orch-server --test pause_resume
  run "M4 gRPC surface"        cargo test -p orch-server --test grpc_surface
  run "M4 runner smoke"        cargo test -p orch-server --test runner_smoke
} | tee "$out/m4-acceptance.txt"

{
  run "seed gate (x2 same seed + different)" \
    cargo test -p orch-server --test seed_gate -- --nocapture
} | tee "$out/seed-gate.txt"

{
  echo "crash points: 11 (AfterWalWrite, AfterDispatch, MidBatchCommit,"
  echo "  AfterCreateNode, BeforeWalDelete, AfterWalDelete,"
  echo "  AfterCommitBeforeCheckpoint, BeforeCheckpointArchive,"
  echo "  AfterCheckpointArchive, BeforeCasPut, AfterCasPut)"
  echo "seeds per point: ${CHAOS_SEEDS_PER_POINT:-5} (override: CHAOS_SEED for the"
  echo "  phases track's fresh-seed spot-check)"
  CHAOS_SEEDS_PER_POINT="${CHAOS_SEEDS_PER_POINT:-5}" \
    run "Tier-1 chaos lattice" cargo test -p orch-server --test chaos_resume -- --nocapture
} | tee "$out/chaos.txt"

echo "evidence written to $out/"
