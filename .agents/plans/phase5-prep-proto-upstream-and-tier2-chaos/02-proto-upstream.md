# Item 1 — Proto Upstream (W1.1–W1.6)

Closes `exploration-orchestrator-777`. Repos: `exploration-orchestrator`
(this one) and the sibling checkout `../control-plane`. We author both
sides; control-plane reviews and lands their half in their repo (their
request, item 3: "they author the build.rs/Cargo changes in the upstream
PR; you review"). **Hard ordering constraint:** everything here lands
before control-plane tags `proto-v0.2.0` or turns on `buf breaking`
against a baseline still containing the placeholder.

Reference files (read before starting):

- Source of truth to upstream:
  `crates/orch-proto/protos/determinism/orchestrator/v1/orchestrator.proto`
- Placeholder to replace:
  `../control-plane/proto/determinism/orchestrator/v1/orchestrator.proto`
  (11 lines: divergent `StartExperimentRequest`, empty service — no
  consumers)
- The pattern to copy: `../control-plane/crates/determinism-proto/`
  (`Cargo.toml` features, `build.rs` `CARGO_FEATURE_*` gating +
  `assert_proto_copies_match`, `lib.rs` `inputsynth`/`scorer` modules and
  the `inputsynth_facade_exposes_required_generated_symbols` test)
- Their request:
  `../control-plane/.agents/requests/phase4-proto-freeze-tag-and-breaking-gate/02-requested-work.md`

## W1.1 — Coordination note (mirror scope + lint posture)

Write
`../control-plane/.agents/requests/phase4-proto-freeze-tag-and-breaking-gate/06-orchestrator-upstream-notes.md`
(their `04-`/`05-` slots stay reserved for resolution/verification per
their `03-verification-offer.md`). Contents:

1. **Mirror scope proposal** per D-P3: the descriptor-equality check
   covers `ExperimentConfig` + `Budgets`, `SelectionConfig`,
   `StagedConfig`, `BurstConfig`, `PlateauConfig`, `LadderConfig`,
   `SchedulingConfig`, `CheckpointConfig` + enums `PruneAction`, `OnGoal`,
   `PolicyKind`, `SchedMode`; orchestrator is source of truth; embed-vs-
   duplicate is their choice (we mildly prefer embedding). Ask for a
   one-line acknowledgement before we merge the upstream.
2. **Lint posture** per D-P2: the three `ignore_only` exemptions we
   ship and why (semantic zero values; API.md-fixed service name;
   `ExperimentStatus` doubling as both a response and an embedded
   message), plus the pre-agreed renumbering escape hatch if they
   insist pre-tag — including its real cost (persisted `config_hash`
   changes; see D-P2).
3. **Sequencing reminder** (their item 3's own words): gates first (with
   placeholder exemptions), then this upstream as a lint-only concern,
   then the tag. Also: their `orchestrator` feature's Rust API changes
   shape (handwritten `StartExperimentRequest` → generated module) — no
   consumer exists in either workspace (verified: nothing outside
   `orch-proto` imports `determinism_proto::orchestrator`), so their
   item-5 versioning note ("bump to 0.3.0 only if…") is not triggered.

Commit (control-plane repo). No code yet.

## W1.2 — Control-plane edits: canonical proto + `orchestrator` feature codegen

All in `../control-plane` (one reviewable commit / PR):

1. **Replace the placeholder** at
   `proto/determinism/orchestrator/v1/orchestrator.proto` with our file,
   byte-for-byte in wire shape. Edit only the header comment: it now
   states this is the canonical copy, authored by exploration-orchestrator
   (API.md §1/§7), upstreamed per bead `exploration-orchestrator-777` /
   plan D4 payback; drop the "authored locally / upstreaming later"
   language. Drop the placeholder's
   `import "determinism/controlplane/v1/resources.proto"` (our file
   doesn't use it); keep `import "google/protobuf/timestamp.proto"`.
2. **Packaged copy:** add byte-identical
   `crates/determinism-proto/proto/determinism/orchestrator/v1/orchestrator.proto`
   (the packaged-copy dir currently holds only inputsynth + scorer).
3. **`crates/determinism-proto/Cargo.toml`:**

   ```toml
   orchestrator = ["common", "controlplane", "dep:prost", "dep:prost-types", "dep:tonic", "dep:tonic-prost"]
   ```

   and add `prost-types = { version = "0.14", optional = true }` to
   `[dependencies]` — the proto uses `google.protobuf.Timestamp`, which
   prost-build maps to `::prost_types::Timestamp` (inputsynth/scorer
   don't use WKTs, which is why the dep is absent today). Keep
   `controlplane` in the feature list even though the generated module no
   longer references `ExperimentSpec` — their item 4 will make
   `controlplane/v1` mirror our config and may re-import; removing it is
   their call, not ours.
4. **`crates/determinism-proto/build.rs`:** add the orchestrator branch,
   symmetrical with the existing two:
   - `println!("cargo:rerun-if-env-changed=CARGO_FEATURE_ORCHESTRATOR");`
   - `let include_orchestrator = std::env::var_os("CARGO_FEATURE_ORCHESTRATOR").is_some();`
   - include it in the early-return check, in the `protos` vec
     (`determinism/orchestrator/v1/orchestrator.proto`), and in
     `assert_proto_copies_match`'s relative-path list (extend the
     function's signature/list accordingly).
5. **`crates/determinism-proto/src/lib.rs`:** replace the handwritten
   `orchestrator` module body with:

   ```rust
   #[cfg(feature = "orchestrator")]
   pub mod orchestrator {
       pub mod v1 {
           tonic::include_proto!("determinism.orchestrator.v1");
       }
   }
   ```

   and add a facade smoke test modeled on
   `inputsynth_facade_exposes_required_generated_symbols`: construct
   `ExplorationOrchestratorClient<tonic::transport::Channel>` (type-size
   probe), a representative `StartExperimentRequest` with a populated
   `ExperimentConfig` (touch `Budgets`, `SelectionConfig`, an enum), a
   `ProgressEvent` with the `edge` oneof set, and assert an enum
   round-trip (`ExperimentState::GoalReached as i32`).
6. Run `cargo build --workspace --all-features && cargo test --workspace
   --all-features` in control-plane, **and** the exploration-orchestrator
   workspace build against the sibling (it consumes the path dep — the
   `orchestrator` feature change must not break `orch-proto` before W1.4
   lands: it won't, because `orch-proto` doesn't re-export
   `determinism_proto::orchestrator` today, but verify).

## W1.3 — Lint conformance (state-dependent)

If control-plane's item 1 (buf gates) has landed by the time W1.2 merges:
add the three `ignore_only` exemptions from D-P2 to their `buf.yaml`
(scoped to the orchestrator file, comment with rationale) and show
`buf lint` green in their CI. If the gates haven't landed yet: put the
exemption stanza + rationale in the W1.1 note instead, explicitly marked
as "fold into your item-1 buf.yaml", and verify locally with a scratch
`buf.yaml` if `buf` is installed (`buf lint proto` from their repo root)
— best-effort, don't install tooling just for this.

## W1.4 — orch-proto cutover (this repo)

1. `crates/orch-proto/src/lib.rs` → the D-P1 re-export shim (keep
   `#![forbid(unsafe_code)]` and update the module docs: both families
   canonical in control-plane, placeholder history one line).
2. **Delete** `crates/orch-proto/protos/` and `crates/orch-proto/build.rs`.
3. `crates/orch-proto/Cargo.toml`: drop `[build-dependencies]` entirely;
   drop direct `prost`, `prost-types`, `tonic-prost` if nothing in the
   shim needs them (the re-export carries its own deps via
   determinism-proto); keep `tonic` only if the shim still references it
   (it shouldn't). Keep
   `determinism-proto = { workspace = true, features = ["orchestrator", "inputsynth"] }`.
4. Rewrite `crates/orch-proto/protos.lock`: both
   `determinism/orchestrator/v1` and `determinism/inputsynth/v1` are
   consumed from the control-plane sibling via `determinism-proto`
   (orchestrator upstreamed 2026-07 per bead 777, control-plane commit
   SHA filled in at resolution time); no local proto sources remain.
5. Verify: `cargo build --workspace && cargo test --workspace
   --all-features && cargo test -p orch-server --test grpc_surface` — all
   green with **zero source changes** outside `crates/orch-proto/`. If
   anything else needs touching, stop and re-check the re-export glob
   (that's the D-P1 contract failing, not a reason to patch consumers).
6. `rg -n "include_proto|protos/" crates/` must show no local
   orchestrator codegen left.

## W1.5 — Cross-repo verification, breaking-gate demo, close the bead

Per the request's `03-verification-offer.md`:

1. Both repos' CI green (ours consumes the canonical location on x86_64
   + aarch64 via the existing sibling checkout; theirs builds/tests the
   new codegen).
2. **Breaking-gate demonstration** — whichever repo lands second runs it;
   realistically us: in control-plane, scratch branch deleting a released
   field (e.g. `ExperimentStats.batch_seq`), push, watch their
   `buf breaking` job go red, delete the branch. Record the run link/
   output in **both** request dirs. Only possible once their gate exists;
   if the gate lands after us, note the pending demo in our resolution
   and let their resolution carry the evidence — do not silently skip it.
   Likewise, if the upstream itself lands before their gates exist
   (their item 3 orders gates → upstream → tag; we can't control their
   timing), disclose the inversion in the resolution — the only hard
   constraint is upstream-before-tag.
3. `bd close exploration-orchestrator-777 -r "determinism.orchestrator.v1
   canonical in control-plane @ <SHA>; orch-proto reduced to re-export;
   no local proto copy remains"` (fill the real SHA).

## W1.6 — EventEnvelope divergence: flag, don't fix

Per the request ("record the divergence and its intended resolution so
observatory M1 doesn't discover it cold"):

1. Create the bead:

   ```bash
   bd create "Reconcile EventEnvelope: runtime struct vs canonical observatory/v1 proto" \
     -d "orch-clients/src/observatory.rs EventEnvelope (postcard payload map, producer_id, ts_logical, seq-excluded-from-hash per D6) diverges from control-plane proto/determinism/observatory/v1/events.proto (payload_json string; no producer_id/ts_logical). Owner of the reconciliation: observatory M1 ingest design — the canonical proto likely needs producer_id + ts_logical and a decision on payload encoding; our emitter then converts at the wire boundary. Do not change orch-clients DTO semantics unilaterally. Flagged from request phase5-prep-proto-upstream-and-tier2-chaos item 1." \
     -p 2 -l analysis -t task
   ```

2. Add the same content as a short section in the W1.1 note file (their
   request dir), so observatory finds it next to the proto-freeze
   context. Explicitly: this is **not** part of the `orchestrator/v1`
   upstream and no code changes here.
