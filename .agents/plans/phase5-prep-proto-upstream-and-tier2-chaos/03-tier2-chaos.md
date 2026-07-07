# Item 2 — Tier-2 True-SIGKILL Chaos Harness (W2.1–W2.7)

Closes `exploration-orchestrator-6ft`. Everything in this repo. The
standard is Tier-1's, across a process boundary: after any number of
real SIGKILLs, the resumed run's committed tree/archive state is
bit-identical to an uninterrupted control run.

Reference files (read before starting):

- Tier-1 harness + comparator: `crates/orch-server/tests/chaos_resume.rs`
  (`store_tree_hash` :43-87, `assert_no_stranded_frontier` :89-121,
  seeds :211-220)
- Crash points: `crates/orch-server/src/experiment.rs` `CrashPoint`
  (:82-111), `CrashPolicy`/`CRASHED_MARKER` (:113-119), the checkpoint /
  WAL-truncation routine (:2208-2310), resume + `load_wal_entries`
  torn-truncation repair (:615-899)
- Fake world state to persist (bead 6ft's checklist):
  `orch-fakes/src/hypervisor.rs` (:44-103 slots + snapshots,
  `reclaim_session` :145), `scorer.rs` (:43-81 archives + batch_cache +
  checkpoints), `snapshot_store.rs` (:21-45 — **metadata holds the
  checkpoint + WAL blobs**), `synth.rs` (:43-68)
- Bin: `bins/orchestratord/src/main.rs` (flags :37-68, standalone
  :113-169, serve :171-218), `simulate.rs` (`SimulatedWorld` :19-40)
- Test fixture conventions: `crates/orch-server/tests/support/mod.rs`
  (`grid_config` :125-149 — the tuning the standalone YAML must mirror)
- Evidence discipline: `scripts/evidence-m3m4.sh`,
  `evidence/phase5-m3-m4/run-manifest.md`

## W2.1 — `crates/orch-simstate`: the crash-consistent journal

New workspace crate (add to root `Cargo.toml` members). `journal.rs`:

- `Journal::create(dir)` / `Journal::open(dir)` on
  `<dir>/journal.v1`. Create fsyncs the directory after creating the
  file (a SIGKILL right after create must not lose the file itself).
- `append(&mut self, record: &JournalRecord) -> io::Result<()>`:
  postcard-encode, write `u32 LE len | u64 LE truncated-blake3(payload) |
  payload`, then `sync_data()`. One frame per call.
- `load(dir) -> io::Result<(Vec<JournalRecord>, LoadStats)>`: scan
  frames; on a short read, length running past EOF, or checksum
  mismatch, **truncate the file to the last good frame boundary**
  (`set_len` + `sync_data`) and stop — torn tails are expected, mid-file
  corruption is not (a checksum failure *followed by* more valid frames
  should panic: that's real corruption, not a torn tail; detect by
  attempting to parse past the bad frame before deciding). `LoadStats`
  records `{frames, truncated_bytes}` for the harness's logging.
- `JournalRecord` v1 enum lives in `records.rs` (next item) with an
  explicit `version` header record written first — refuse to load a
  journal whose header version ≠ 1.
- Unit tests: round-trip; torn tail at every byte offset of the final
  frame (proptest or an exhaustive loop over `0..frame_len`) reloads to
  exactly the prefix; a bit-flip mid-file panics; empty file loads empty.

Env hook for W2.4's forced-torn-write kills (per D-T3): if
`ORCH_SIM_TORN_AT=<kind>:<nth>` matches the record being appended
(`wal-append` = `put_metadata` on a WAL key, `ckpt-put` = `put_metadata`
on a checkpoint key — match via `MetadataKey` helpers), write only a
prefix of the frame (length header + half the payload), `sync_data()`,
print `TIER2_CHAOS_HANG kind=<kind>` to stdout, flush stdout, and park
the thread forever. Key-kind detection needs the record, so the hook
lives in `append` with the match done by the caller passing a
`RecordKind` tag (keep `Journal` itself dumb about client types).

## W2.2 — `PersistentWorld`: journaling wrappers + replay

`orch-simstate/src/world.rs` + `records.rs`:

- `JournalRecord` variants: one per mutating method of the four
  `orch-clients` sync traits (enumerate from the trait definitions —
  hypervisor create/fork/run/destroy etc., scorer submit-batch /
  checkpoint-archive / restore / replay-commits / rebin, store
  create-node / update-node / put-metadata / delete-metadata, synth
  load-macro-pack / load-experiment-config …), each carrying the request
  DTO plus `response_digest: u64` (truncated blake3 of the
  postcard-encoded `Result`-payload). Plus `Header { version }` and
  `ReclaimSession`. Read-only methods (queries, list/watch, propose) are
  **not** journaled. When a method's mutability is unclear from the
  trait, journal it and note it in a comment.
- Request/response DTOs need `Serialize`/`Deserialize`. Preferred: add
  derives to `orch-clients` DTOs behind a `serde` cargo feature
  (orch-clients is transport-free; postcard/serde is already the repo's
  canonical encoding — see `orch-checkpoint`). If some DTO resists
  (borrowed data, non-serde field), mirror just that one as a local
  struct with `From`/`Into` — don't fork the whole DTO layer.
- `Persistent<Fake>` wrappers implement the same sync client traits,
  holding `(inner_fake, Arc<Mutex<Journal>>, kind_tag)`. Mutating
  methods: build record → `journal.append` (write-ahead) → apply to
  inner → digest the response into… note the digest is *of* the
  response, which doesn't exist at append time. Resolution: append the
  record with `response_digest = 0`, apply, then **amend is forbidden**
  (append-only) — so instead journal the digest in a paired follow-up
  record? No — keep it simple and honest: the record carries the request
  only; the **response digest goes in a trailing `Applied { digest }`
  frame appended (no fsync needed — advisory) after the op applies.** On
  replay, re-invoke the op; if the *next* frame is its `Applied` record,
  compare digests and panic on mismatch (naming the record index); if
  the journal ends before the `Applied` frame (crash between append and
  apply), the re-invoked result is authoritative — exactly the
  "executed, response lost" semantics from D-T2. Loader pairs each op
  frame with an optional immediately-following `Applied` frame.
- `PersistentWorld::create(dir)` — fresh fakes
  (`FakeHypervisor::with_world(GridWorld::three_room())` etc., matching
  `simulate.rs`), new journal. `PersistentWorld::reload(dir)` — fresh
  fakes, `Journal::load`, re-invoke ops in order (digest-checking per
  above), then invoke + append `ReclaimSession` per D-T4. Both return
  the same shape `SimulatedWorld` uses (`SyncAdapter<Persistent<…>>` ×4).
- `compare.rs`: extract Tier-1's comparator so both tiers share it —
  `store_tree_hash(&InMemorySnapshotStore) -> [u8; 32]` (blake3 over
  sorted nodes: node_id, parent, synth state_hash, progress_score,
  cell_key; dense-id assertion), `assert_no_stranded_frontier`, plus new
  `scorer_archive_fingerprint(&FakeScorer, experiment_id) -> [u8; 32]`
  (blake3 over sorted cell counts + `archive_seq` + feature-map/program
  hashes) — Tier-2's bar says tree **/archive** state. Rework
  `chaos_resume.rs` to import the shared comparator (dev-dependency on
  `orch-simstate`) instead of its private copy; its assertions must not
  change.
- Unit test: drive a scripted op sequence (mirror
  `orch-fakes/tests/search_loop.rs`'s shape) against
  `PersistentWorld::create`, reload into a second world, assert equal
  tree hash + archive fingerprint + identical responses to a battery of
  read queries. Then re-drive the tail after reload to prove the world
  is live, not just readable.

## W2.3 — `orchestratord`: `--state-dir` + chaos hang hook

`bins/orchestratord`:

- New flag `--state-dir <dir>`, valid with both `--simulate` and
  `--experiment`. Present ⇒ `SimulatedWorld` is backed by
  `PersistentWorld::reload(dir)` if `<dir>/journal.v1` exists, else
  `::create(dir)`. Absent ⇒ today's in-memory world (zero behavior
  change; Tier-1 and all existing tests unaffected).
- `ORCH_CHAOS_HANG_AT=<CrashPoint>:<nth>` (per D-T3): parsed only in
  `--experiment` mode; builds the hang-and-marker `CrashPolicy`
  (`TIER2_CHAOS_HANG point=<point>` on stdout, flush, park) passed where
  `None` goes today (`main.rs:151`). `CrashPoint` needs `FromStr` (or a
  small match) — add it next to the enum in `orch-server`. Document both
  env vars in the bin's module docs as test-only chaos hooks.
- The gRPC-served path keeps `CrashPolicy = None` (D-T4); it still gets
  `--state-dir` so the resume smoke can relaunch against the same dir.

## W2.4 — The harness: `bins/orchestratord/tests/tier2_chaos.rs`

Uses `env!("CARGO_BIN_EXE_orchestratord")`; `std::process::Command`,
SIGKILL via `Child::kill()` (SIGKILL on unix) — never SIGTERM. Per seed
(seeds: `0x5EED + i*7` matching Tier-1; count from `TIER2_SEEDS`,
default 1 in CI, 5 in the evidence lane):

1. Write the standalone YAML once (grid tuning from
   `support::grid_config`, translated to the sparse wire-YAML field
   names `wire_config_from_yaml`/`orch-server/src/config.rs` accepts,
   with the seed substituted).
2. **Control run:** fresh state-dir A, run to exit 0, reload A via
   `PersistentWorld::reload`, record tree hash + archive fingerprint +
   goal-node set + persisted checkpoint `status == GoalReached`.
3. **Lattice rounds:** for each of the 11 `CrashPoint`s — fresh
   state-dir B; launch with `ORCH_CHAOS_HANG_AT=<point>:<nth>` (vary nth
   over incarnations like Tier-1's `CrashOnce` does: `1 + (k + seed) % 3`);
   wait for the `TIER2_CHAOS_HANG` marker on stdout (bounded by a
   generous timeout — a missing marker after clean exit means the point
   wasn't reached this incarnation: relaunch without the hook counts it
   as converged, mirroring Tier-1's ≤3-crash policy); SIGKILL; relaunch
   against the same dir. After the final clean exit: reload B, assert
   tree hash + archive fingerprint + goal nodes == control's, and
   `assert_no_stranded_frontier`.
4. **Forced torn writes:** two rounds (`ORCH_SIM_TORN_AT=wal-append:2`,
   `ckpt-put:1`) — marker, SIGKILL, relaunch loop as above. These are
   the accept bar's "mid-WAL-append" and "mid-checkpoint-write"; the
   relaunch must log a nonzero `truncated_bytes` (surface `LoadStats`
   via a tracing line the harness greps) proving the torn-tail path
   actually fired.
5. **Random kills:** `TIER2_RANDOM_KILLS` rounds — launch with no hooks,
   sleep `rand(50..800)` ms, SIGKILL if still alive (an early clean exit
   doesn't count toward the kill quota; loop until it does), relaunch to
   completion, compare as above.
6. **Served-gRPC smoke** (once per suite, not per seed): `--simulate
   --state-dir C --listen 127.0.0.1:0`… `--listen` needs a concrete
   port — pick an ephemeral port in the harness and pass it. Start an
   experiment over gRPC (`StartExperiment`), random-kill, relaunch,
   `StartExperiment(resume_if_exists = true)` must return `resumed_at_
   batch_seq > 0` and the run must reach `GoalReached` via
   `GetExperimentStatus` polling; compare state-dir C against a control
   as above. (The tonic client comes via `orch-proto` — dev-dependency
   of the bin.)

Count every SIGKILL delivered; the evidence lane asserts total ≥ 50 and
prints the per-class breakdown. Sum with defaults: 5 seeds × (11 lattice
+ 2 torn) = 65 forced kills before random ones — the bar clears
structurally, but keep the assertion so a future default change can't
silently sink it. Kill-loop safety: cap incarnations per round (64, as
Tier-1) and fail loudly on non-convergence.

## W2.5 — The demonstrated negative

A comparator that can't detect a real divergence proves nothing (accept
bar 3). In `orch-simstate`, `PersistentWorld::reload_broken(dir,
BreakMode::DropWal)` — identical to `reload` except it **skips replaying
`put_metadata` records whose key is a WAL key** (the documented
mutation: "WAL replay skipped" — the resumed runner sees the checkpoint
but no surviving WAL entries).

Harness test `negative_control_detects_wal_loss`: seed 0x5EED, kill at
`AfterWalWrite:1` (a point where the WAL entry *matters* — the batch was
dispatched but not committed), relaunch **via a broken world**: this
needs the bin to reload broken, so `--state-dir` gains a test-only env
`ORCH_SIM_BREAK=drop-wal` honored on reload (documented alongside the
other hooks). Run to completion, reload the dir, and assert the tree
hash **differs** from control (or the run fails/wedges — assert
"comparator would have failed", i.e. `hash != control || outcome !=
GoalReached`). Also assert the *unbroken* reload of a copy of the same
pre-relaunch dir still converges to equality — pinning that the
divergence is caused by the mutation, not the kill point. Record the
exact mutation + observed divergence in the evidence file.

## W2.6 — Evidence + CI

- `scripts/evidence-tier2.sh`, same discipline as `evidence-m3m4.sh`:
  manifest header (date UTC, uname, rustc, `git rev-parse HEAD`, seeds,
  kill counts) to `evidence/phase5-tier2-chaos/run-manifest.md`; runs
  the harness with `TIER2_SEEDS=5 TIER2_RANDOM_KILLS=<n>` and tees
  trimmed output (`^test |test result|TIER2_` lines) to
  `evidence/phase5-tier2-chaos/tier2-chaos.txt` and the negative test to
  `negative-control.txt`. The harness prints a `TIER2_SUMMARY seeds=…
  kills=… lattice=… torn=… random=…` line the evidence file must contain.
- CI (`.github/workflows/ci.yaml`): a new job `tier2-chaos-smoke` (both
  matrix arches, after the main job or parallel) running
  `cargo test -p orchestratord --test tier2_chaos` at the reduced
  defaults from D-T5. Keep it a separate job so its runtime is visible
  and its failure names itself. Measure the full-matrix runtime during
  W2.4; if < ~10 min, run the full matrix in CI instead and say so in
  the resolution (D-T5).

## W2.7 — D5 trail note, bead, resolution

1. Append a dated addendum to
   `.agents/requests/phase5-entry-m3-m4-runner-on-fakes/04-resolution.md`
   under "Disclosed follow-up beads": Tier-2 gap closed by this plan,
   pointing at the harness, the evidence dir, and bead 6ft's closure —
   so the Phase 5 gate-run checklist cites the closure, not the descope
   (accept bar 4).
2. `bd close exploration-orchestrator-6ft -r "Tier-2 true-SIGKILL harness
   landed: orch-simstate journal + PersistentWorld, orchestratord
   --state-dir, bins/orchestratord/tests/tier2_chaos.rs; evidence/
   phase5-tier2-chaos/ (≥5 seeds, ≥50 kills, 11 lattice + torn-WAL +
   torn-ckpt + random, negative control demonstrated)"`.
3. Write `.agents/requests/phase5-prep-proto-upstream-and-tier2-chaos/
   04-resolution.md` per `04-verification.md`'s handback shape.
