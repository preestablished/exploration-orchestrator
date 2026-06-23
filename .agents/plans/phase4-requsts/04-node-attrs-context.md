# 04 Node Attrs Envelope and NodeContext Reconstruction

Goal: persist enough child metadata to reconstruct future `NodeContext.parent_burst` and `NodeContext.sibling_bursts` without letting the synthesizer query snapshot-store.

Attrs ownership:

- The attrs postcard blob is orchestrator-private.
- Put encode/decode helpers in `orch-driver`, not `orch-core`; the envelope depends on `orch-clients::input_synth` DTOs.
- Keep the snapshot-store DTO `NodeAttrs` opaque.

Envelope shape:

- Define a versioned envelope with a magic/version field, for example `OrchNodeAttrsV1`.
- Include at least:
  - `created_by_burst: Option<ProvenancedBurst>` for non-root committed children
  - `config_fingerprint: Option<ConfigFingerprint>`
  - `decoded_features: BTreeMap<String, FiniteF64>` for `feat/<name>` values
  - `frame_counter: FrameCount`
  - `state_hash: StateHash`
  - `cell_key: CellKey`
  - `stage: Stage`
  - `score: Score`
  - `novelty: Novelty`
  - `recent_inputs: Option<Burst>` or an explicit recent-input-tail metadata struct
- Root nodes may encode no `created_by_burst`, but should still encode state, score, novelty, features, and frame counter.
- Add decode errors for missing required fields, unknown versions, and malformed postcard bytes.

Recent input tail:

- Prefer storing a bounded `recent_inputs` tail in attrs so context assembly does not need to walk the full root-to-node path.
- For pad bursts, append parent tail plus child burst and cap at the documented frame window.
- For event bursts, append parent tail plus child events and cap at the documented event/token window.
- If the first implementation cannot reconstruct recent inputs safely, store `None` deliberately and document that limitation in the helper and smoke test. Do not leave the behavior implicit.

Context assembly helper:

- Add a helper such as `build_input_synth_node_context(store, experiment_id, node_id, limits)` in `orch-driver`.
- Steps:
  - load the selected node
  - decode selected attrs
  - copy `node_id`, `parent_node_id`, `snapshot_ref`, `state_hash`, `cell_key`, `stage`, `depth`, `frame_counter`, `node_score`, `novelty`, decoded features, frame embedding empty, and recent input tail
  - set `parent_burst` to the selected node's `created_by_burst`
  - if the node has a parent, load committed children of that same parent
  - exclude the selected node from siblings
  - include only children whose attrs decode with `created_by_burst`
  - order sibling bursts by `node_id` ascending
  - cap siblings by a documented deterministic limit if needed
  - compute `score_delta = child.progress_score - parent.progress_score`
- Use `FiniteF64::new` for score deltas and treat non-finite results as data loss.

Commit integration:

- When committing a child created from a synth proposal, encode the full `ProvenancedBurst`, response `config_fingerprint`, decoded score features, frame counter, score, novelty, and recent input tail into `CreateNodeRequest.attrs`.
- The response fingerprint guard from `03-bringup-seeds-fingerprints.md` must run before attrs are built for commit.

Definition of done:

- Postcard encode/decode round-trip tests cover root and child attrs.
- Unknown envelope version and malformed bytes fail cleanly.
- A child expansion can build `parent_burst` from its own committed attrs.
- A sibling expansion can build `sibling_bursts` from same-parent committed children with exact deterministic score deltas.

