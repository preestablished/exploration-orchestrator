#!/usr/bin/env bash
# Evidence pass for the Tier-2 true-SIGKILL chaos harness (plan W2.6, bead
# exploration-orchestrator-6ft). Full matrix: TIER2_SEEDS seeds x (all 11
# lattice points + both torn-write kinds + TIER2_RANDOM_KILLS randoms),
# every kill a real SIGKILL on the orchestratord process; asserts the
# total kill count >= TIER2_MIN_KILLS. CHAOS_SEED overrides to a single
# seed (the phases track's fresh-seed spot-check).
#
# Exit status reflects the tests: any non-zero `cargo test` (including the
# in-test kill-quota gate) makes this script exit non-zero, so automation
# keying on the exit code is not misled by a green-looking manifest.
set -euo pipefail
cd "$(dirname "$0")/.."
out=evidence/phase5-tier2-chaos
mkdir -p "$out"

seeds="${TIER2_SEEDS:-5}"
randoms="${TIER2_RANDOM_KILLS:-3}"
min_kills="${TIER2_MIN_KILLS:-50}"

{
  echo "# Evidence run manifest"
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "host: $(uname -a)"
  echo "toolchain: $(rustc --version)"
  echo "commit: $(git rev-parse HEAD)"
  echo "seeds: $seeds (0x5EED + i*7${CHAOS_SEED:+; overridden by CHAOS_SEED=$CHAOS_SEED})"
  echo "lattice: full (all 11 CrashPoints) + torn wal-append + torn ckpt-put"
  echo "random kills per seed: $randoms"
  echo "kill quota asserted: >= $min_kills"
} > "$out/run-manifest.md"

status_file="$(mktemp)"
trap 'rm -f "$status_file"' EXIT

# run <display-name> <cmd...> — prints trimmed output, records the *cargo*
# exit code (not grep's no-match, and never masked by `|| true`).
run() {
  local name="$1"; shift
  echo "== $name =="
  set +e
  "$@" 2>&1 | grep -E "^test |test result|TIER2_"
  local rc=${PIPESTATUS[0]}
  set -e
  if [ "$rc" -ne 0 ]; then
    echo "== $name FAILED (cargo exit $rc) =="
  fi
  echo "$rc" >> "$status_file"
}

{
  TIER2_ENABLE=1 TIER2_SEEDS="$seeds" TIER2_LATTICE=full TIER2_RANDOM_KILLS="$randoms" \
  TIER2_MIN_KILLS="$min_kills" \
    run "Tier-2 kill matrix + gRPC resume smoke" \
    cargo test -p orchestratord --test tier2_chaos -- --nocapture \
      tier2_kill_matrix_resumes_bit_identically grpc_served_resume_survives_sigkill
} | tee "$out/tier2-chaos.txt"

{
  TIER2_ENABLE=1 \
    run "Tier-2 negative control (perturb-node replay mutation)" \
    cargo test -p orchestratord --test tier2_chaos -- --nocapture \
      negative_control_detects_divergence
} | tee "$out/negative-control.txt"

if grep -qvE '^0$' "$status_file"; then
  echo "evidence run FAILED — a test invocation exited non-zero (see $out/*.txt)" >&2
  exit 1
fi

echo "evidence written to $out/"
