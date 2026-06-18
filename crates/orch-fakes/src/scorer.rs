//! Fake state scorer surface for deterministic search-loop tests.
//!
//! This fake satisfies [`orch_clients::scorer::StateScorerClient`] over the
//! synthetic grid-world capture format used by in-repository fake services.

use std::collections::{BTreeMap, BTreeSet};

use orch_clients::{
    scorer::{
        ArchiveUpdateMode, ArtifactSource, CheckpointArchiveRequest, CheckpointArchiveResponse,
        ComponentScore, DecodedValue, Digest32, FrameSpec, Framebuffer, ItemError, ItemErrorKind,
        LoadFeatureMapRequest, LoadFeatureMapResponse, LoadScoringProgramRequest,
        LoadScoringProgramResponse, NoveltyDetail, ReplayCommitsRequest, ReplayCommitsResponse,
        RestoreArchiveRequest, RestoreArchiveResponse, ScoreBatchRequest, ScoreBatchResponse,
        ScoreResult, StateScorerClient,
    },
    ClientError, ClientErrorKind, ClientResult,
};
use orch_core::types::{CellKey, FiniteF64, Novelty, Score, Stage, StateHash};
use serde::{Deserialize, Serialize};

use crate::grid::{GridState, Room, BOSS_MAX_HP, GRID_HEIGHT, GRID_WIDTH};

pub const MAX_SCORE_BATCH_ITEMS: usize = 256;
pub const GRID_FEATURE_BYTES_LEN: u32 = 5;

const COMPONENT_NAMES: [&str; 5] = [
    "stage/start",
    "stage/key",
    "stage/boss",
    "shape/x-progress",
    "penalty/prune",
];
const STAGE_NAMES: [&str; 4] = ["start", "key", "boss", "credits"];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FakeScorer {
    experiments: BTreeMap<String, ExperimentArchive>,
    checkpoints: BTreeMap<CheckpointKey, ArchiveSnapshot>,
}

impl FakeScorer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn archive_seq(&self, experiment_id: &str) -> Option<u64> {
        self.experiments
            .get(experiment_id)
            .map(|archive| archive.archive_seq)
    }

    fn archive_mut(&mut self, experiment_id: &str) -> &mut ExperimentArchive {
        self.experiments
            .entry(experiment_id.to_owned())
            .or_default()
    }

    fn loaded_archive(&self, experiment_id: &str) -> ClientResult<&ExperimentArchive> {
        let archive = self.experiments.get(experiment_id).ok_or_else(|| {
            ClientError::new(
                ClientErrorKind::NotFound,
                format!("unknown experiment_id '{experiment_id}'"),
            )
        })?;
        archive.ensure_loaded()?;
        Ok(archive)
    }

    fn loaded_archive_mut(&mut self, experiment_id: &str) -> ClientResult<&mut ExperimentArchive> {
        let archive = self.experiments.get_mut(experiment_id).ok_or_else(|| {
            ClientError::new(
                ClientErrorKind::NotFound,
                format!("unknown experiment_id '{experiment_id}'"),
            )
        })?;
        archive.ensure_loaded()?;
        Ok(archive)
    }
}

impl StateScorerClient for FakeScorer {
    fn load_feature_map(
        &mut self,
        request: LoadFeatureMapRequest,
    ) -> ClientResult<LoadFeatureMapResponse> {
        let feature_map_hash = hash_artifact_source(b"feature-map", &request.source);
        let feature_bytes_len = layout_len(&request.layout)?;
        let layout_signature = layout_signature(&request.layout);
        let archive = self.archive_mut(&request.experiment_id);

        if let Some(loaded_hash) = archive.feature_map_hash {
            if loaded_hash != feature_map_hash {
                if !request.rebin && !archive.is_empty() {
                    return Err(ClientError::new(
                        ClientErrorKind::FailedPrecondition,
                        "feature map differs from non-empty archive; use rebin",
                    ));
                }
                if request.rebin && archive.layout_signature != Some(layout_signature) {
                    return Err(ClientError::new(
                        ClientErrorKind::FailedPrecondition,
                        "rebin requires identical compiled layout",
                    ));
                }
                if request.rebin {
                    archive.cell_counts.clear();
                }
            }
        }

        archive.feature_map_hash = Some(feature_map_hash);
        archive.feature_bytes_len = feature_bytes_len;
        archive.layout_signature = Some(layout_signature);
        archive.frame = request.frame;

        Ok(LoadFeatureMapResponse {
            feature_map_hash,
            field_count: GRID_FEATURE_BYTES_LEN,
            feature_bytes_len,
            warnings: request
                .rebin
                .then(|| "rebin reset fake scorer cell counts".to_owned())
                .into_iter()
                .collect(),
        })
    }

    fn load_scoring_program(
        &mut self,
        request: LoadScoringProgramRequest,
    ) -> ClientResult<LoadScoringProgramResponse> {
        let archive = self.archive_mut(&request.experiment_id);
        if archive.feature_map_hash.is_none() {
            return Err(ClientError::new(
                ClientErrorKind::FailedPrecondition,
                "feature map must be loaded before scoring program",
            ));
        }

        let program_hash = hash_artifact_source(b"scoring-program", &request.source);
        if let Some(loaded_hash) = archive.program_hash {
            if loaded_hash != program_hash && !archive.is_empty() {
                return Err(ClientError::new(
                    ClientErrorKind::FailedPrecondition,
                    "scoring program differs from non-empty archive",
                ));
            }
        }
        archive.program_hash = Some(program_hash);

        Ok(LoadScoringProgramResponse {
            program_hash,
            component_names: COMPONENT_NAMES.iter().map(ToString::to_string).collect(),
            goal_expr: "room == boss && boss_hp == 0 && x == 4 && y == 0".to_owned(),
            warnings: Vec::new(),
            stage_names: STAGE_NAMES.iter().map(ToString::to_string).collect(),
        })
    }

    fn score_batch(&mut self, request: ScoreBatchRequest) -> ClientResult<ScoreBatchResponse> {
        if request.states.is_empty() || request.states.len() > MAX_SCORE_BATCH_ITEMS {
            return Err(ClientError::new(
                ClientErrorKind::InvalidRequest,
                format!(
                    "states length must be in 1..={MAX_SCORE_BATCH_ITEMS}, got {}",
                    request.states.len()
                ),
            ));
        }

        let archive = self.loaded_archive_mut(&request.experiment_id)?;
        if let Some(cached) = archive.batch_cache.get(&request.client_batch_id) {
            return Ok(cached.clone());
        }

        let seen_before = archive.seen_hashes.clone();
        let cell_counts_before = archive.cell_counts.clone();
        let mut results = Vec::with_capacity(request.states.len());
        let mut scored_states = Vec::new();
        for input in &request.states {
            let scored = score_state_input(
                input.node_ref.clone(),
                &input.feature_bytes,
                input.framebuffer.as_ref(),
                input.fb_meta.as_ref(),
                archive,
                &seen_before,
                &cell_counts_before,
                request.return_decoded,
            );
            if let Some(committed) = scored.committed {
                scored_states.push(committed);
            }
            results.push(scored.result);
        }

        let mut archive_seq = archive.archive_seq;
        if request.archive_update == ArchiveUpdateMode::ScoreAndInsert {
            for committed in scored_states {
                if archive.seen_hashes.insert(committed.state_hash) {
                    *archive.cell_counts.entry(committed.cell_key).or_default() += 1;
                }
            }
            archive_seq = archive_seq.saturating_add(1);
            archive.archive_seq = archive_seq;
        }

        let response = ScoreBatchResponse {
            client_batch_id: request.client_batch_id.clone(),
            archive_seq,
            results,
        };
        if request.archive_update == ArchiveUpdateMode::ScoreAndInsert {
            archive
                .batch_cache
                .insert(request.client_batch_id, response.clone());
        }
        Ok(response)
    }

    fn checkpoint_archive(
        &mut self,
        request: CheckpointArchiveRequest,
    ) -> ClientResult<CheckpointArchiveResponse> {
        let archive = self.loaded_archive(&request.experiment_id)?;
        let snapshot = archive.snapshot()?;
        let archive_hash = snapshot.hash();
        let archive_ref = archive_ref(archive_hash);
        let key = CheckpointKey {
            experiment_id: request.experiment_id,
            checkpoint_id: request.checkpoint_id,
            archive_ref: archive_ref.clone(),
        };
        self.checkpoints.insert(key, snapshot.clone());

        Ok(CheckpointArchiveResponse {
            archive_ref,
            archive_hash,
            archive_seq: snapshot.archive_seq,
            cell_count: snapshot.cell_counts.len() as u64,
            phash_count: 0,
            embedding_count: 0,
            blob_bytes: postcard::to_allocvec(&snapshot)
                .expect("snapshot serializes")
                .len() as u64,
        })
    }

    fn restore_archive(
        &mut self,
        request: RestoreArchiveRequest,
    ) -> ClientResult<RestoreArchiveResponse> {
        let key = CheckpointKey {
            experiment_id: request.experiment_id.clone(),
            checkpoint_id: request.checkpoint_id,
            archive_ref: request.archive_ref,
        };
        let snapshot = self.checkpoints.get(&key).cloned().ok_or_else(|| {
            ClientError::new(ClientErrorKind::NotFound, "archive checkpoint not found")
        })?;
        let archive = self.archive_mut(&request.experiment_id);
        if archive.feature_map_hash != Some(snapshot.feature_map_hash)
            || archive.program_hash != Some(snapshot.program_hash)
        {
            return Err(ClientError::new(
                ClientErrorKind::FailedPrecondition,
                "loaded scorer artifacts do not match archive checkpoint bindings",
            ));
        }
        archive.restore(snapshot.clone());

        Ok(RestoreArchiveResponse {
            archive_seq: snapshot.archive_seq,
            cell_count: snapshot.cell_counts.len() as u64,
            phash_count: 0,
            embedding_count: 0,
            bound_feature_map_hash: snapshot.feature_map_hash,
            bound_scoring_program_hash: snapshot.program_hash,
        })
    }

    fn replay_commits(
        &mut self,
        request: ReplayCommitsRequest,
    ) -> ClientResult<ReplayCommitsResponse> {
        let archive = self.loaded_archive_mut(&request.experiment_id)?;
        let mut applied = 0u64;
        let mut skipped = 0u64;

        for state in request.states {
            if archive.seen_hashes.insert(state.state_hash) {
                *archive.cell_counts.entry(state.cell_key).or_default() += 1;
                applied += 1;
            } else {
                skipped += 1;
            }
        }

        Ok(ReplayCommitsResponse { applied, skipped })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct ExperimentArchive {
    feature_map_hash: Option<Digest32>,
    program_hash: Option<Digest32>,
    feature_bytes_len: u32,
    layout_signature: Option<Digest32>,
    frame: Option<FrameSpec>,
    archive_seq: u64,
    seen_hashes: BTreeSet<StateHash>,
    cell_counts: BTreeMap<CellKey, u32>,
    batch_cache: BTreeMap<String, ScoreBatchResponse>,
}

impl ExperimentArchive {
    fn ensure_loaded(&self) -> ClientResult<()> {
        if self.feature_map_hash.is_none() || self.program_hash.is_none() {
            return Err(ClientError::new(
                ClientErrorKind::FailedPrecondition,
                "feature map and scoring program must be loaded",
            ));
        }
        Ok(())
    }

    fn is_empty(&self) -> bool {
        self.archive_seq == 0 && self.seen_hashes.is_empty() && self.cell_counts.is_empty()
    }

    fn snapshot(&self) -> ClientResult<ArchiveSnapshot> {
        Ok(ArchiveSnapshot {
            feature_map_hash: self.feature_map_hash.ok_or_else(|| {
                ClientError::new(
                    ClientErrorKind::FailedPrecondition,
                    "feature map not loaded",
                )
            })?,
            program_hash: self.program_hash.ok_or_else(|| {
                ClientError::new(
                    ClientErrorKind::FailedPrecondition,
                    "scoring program not loaded",
                )
            })?,
            feature_bytes_len: self.feature_bytes_len,
            layout_signature: self.layout_signature.ok_or_else(|| {
                ClientError::new(ClientErrorKind::FailedPrecondition, "layout not loaded")
            })?,
            frame: self.frame,
            archive_seq: self.archive_seq,
            seen_hashes: self.seen_hashes.clone(),
            cell_counts: self.cell_counts.clone(),
            batch_cache: self.batch_cache.clone(),
        })
    }

    fn restore(&mut self, snapshot: ArchiveSnapshot) {
        self.feature_map_hash = Some(snapshot.feature_map_hash);
        self.program_hash = Some(snapshot.program_hash);
        self.feature_bytes_len = snapshot.feature_bytes_len;
        self.layout_signature = Some(snapshot.layout_signature);
        self.frame = snapshot.frame;
        self.archive_seq = snapshot.archive_seq;
        self.seen_hashes = snapshot.seen_hashes;
        self.cell_counts = snapshot.cell_counts;
        self.batch_cache = snapshot.batch_cache;
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct CheckpointKey {
    experiment_id: String,
    checkpoint_id: String,
    archive_ref: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ArchiveSnapshot {
    feature_map_hash: Digest32,
    program_hash: Digest32,
    feature_bytes_len: u32,
    layout_signature: Digest32,
    frame: Option<FrameSpec>,
    archive_seq: u64,
    seen_hashes: BTreeSet<StateHash>,
    cell_counts: BTreeMap<CellKey, u32>,
    batch_cache: BTreeMap<String, ScoreBatchResponse>,
}

impl ArchiveSnapshot {
    fn hash(&self) -> Digest32 {
        let bytes = postcard::to_allocvec(self).expect("snapshot serializes");
        Digest32::new(*blake3::hash(&bytes).as_bytes())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScoredCommittedState {
    state_hash: StateHash,
    cell_key: CellKey,
}

#[derive(Clone, Debug, PartialEq)]
struct ScoredItem {
    result: ScoreResult,
    committed: Option<ScoredCommittedState>,
}

#[must_use]
pub fn encode_grid_features(state: GridState) -> Vec<u8> {
    vec![state.room.id(), state.x, state.y, state.keys, state.boss_hp]
}

fn decode_grid_features(bytes: &[u8]) -> Result<GridState, ItemError> {
    if bytes.len() != GRID_FEATURE_BYTES_LEN as usize {
        return Err(ItemError {
            kind: ItemErrorKind::FeatureLenMismatch,
            detail: format!(
                "expected {} grid feature bytes, got {}",
                GRID_FEATURE_BYTES_LEN,
                bytes.len()
            ),
        });
    }

    let room = match bytes[0] {
        0 => Room::Start,
        1 => Room::KeyVault,
        2 => Room::Boss,
        value => {
            return Err(ItemError {
                kind: ItemErrorKind::DecodeFailed,
                detail: format!("invalid room id {value}"),
            });
        }
    };
    let state = GridState {
        room,
        x: bytes[1],
        y: bytes[2],
        keys: bytes[3],
        boss_hp: bytes[4],
    };
    if state.x >= GRID_WIDTH || state.y >= GRID_HEIGHT || state.boss_hp > BOSS_MAX_HP {
        return Err(ItemError {
            kind: ItemErrorKind::DecodeFailed,
            detail: "grid state fields out of range".to_owned(),
        });
    }

    Ok(state)
}

#[allow(clippy::too_many_arguments)]
fn score_state_input(
    node_ref: String,
    feature_bytes: &[u8],
    framebuffer: Option<&Framebuffer>,
    fb_meta: Option<&orch_clients::scorer::FramebufferMeta>,
    archive: &ExperimentArchive,
    seen_before: &BTreeSet<StateHash>,
    cell_counts_before: &BTreeMap<CellKey, u32>,
    return_decoded: bool,
) -> ScoredItem {
    if let Some(error) = validate_framebuffer(framebuffer, fb_meta, archive.frame.as_ref()) {
        return ScoredItem {
            result: error_result(node_ref, error),
            committed: None,
        };
    }

    let state = match decode_grid_features(feature_bytes) {
        Ok(state) => state,
        Err(error) => {
            return ScoredItem {
                result: error_result(node_ref, error),
                committed: None,
            };
        }
    };
    let state_hash = state.state_hash();
    let cell_key = state.cell_key();
    let duplicate = seen_before.contains(&state_hash);
    let cell_count = cell_counts_before.get(&cell_key).copied().unwrap_or(0);
    let count_novelty = novelty(1.0 / f64::from(cell_count + 1).sqrt());
    let visual_novelty = framebuffer.map_or(novelty(0.0), |_| novelty(1.0));
    let progress_score = progress_score(state);

    ScoredItem {
        result: ScoreResult {
            node_ref,
            error: None,
            progress_score,
            novelty_score: count_novelty,
            state_hash,
            goal_hit: state.goal_reached(),
            duplicate,
            stage: stage(state),
            prune: prune(state),
            decoded: return_decoded
                .then(|| decoded_values(state))
                .unwrap_or_default(),
            component_breakdown: component_breakdown(state),
            novelty_detail: NoveltyDetail {
                count_novelty,
                cell_key,
                cell_count,
                visual_novelty,
                phash: visual_novelty_hash(state, framebuffer),
                phash_min_hamming: framebuffer.map_or(64, |_| 0),
                rnd_error: finite(0.0),
                knn_distance: finite(0.0),
            },
        },
        committed: Some(ScoredCommittedState {
            state_hash,
            cell_key,
        }),
    }
}

fn validate_framebuffer(
    framebuffer: Option<&Framebuffer>,
    fb_meta: Option<&orch_clients::scorer::FramebufferMeta>,
    frame: Option<&FrameSpec>,
) -> Option<ItemError> {
    match framebuffer {
        Some(Framebuffer::BlobRef(_)) => Some(ItemError {
            kind: ItemErrorKind::FbRefUnsupported,
            detail: "fake scorer does not resolve framebuffer blobs".to_owned(),
        }),
        Some(_) => {
            let Some(expected) = frame else {
                return Some(ItemError {
                    kind: ItemErrorKind::FbMetaMismatch,
                    detail: "framebuffer supplied without loaded frame spec".to_owned(),
                });
            };
            let Some(meta) = fb_meta else {
                return Some(ItemError {
                    kind: ItemErrorKind::FbMetaMismatch,
                    detail: "framebuffer metadata missing".to_owned(),
                });
            };
            (meta.width != expected.width
                || meta.height != expected.height
                || meta.format != expected.format)
                .then(|| ItemError {
                    kind: ItemErrorKind::FbMetaMismatch,
                    detail: "framebuffer metadata does not match loaded frame spec".to_owned(),
                })
        }
        None => None,
    }
}

fn error_result(node_ref: String, error: ItemError) -> ScoreResult {
    ScoreResult {
        node_ref,
        error: Some(error),
        progress_score: score(0.0),
        novelty_score: novelty(0.0),
        state_hash: StateHash::new([0; 32]),
        goal_hit: false,
        duplicate: false,
        stage: Stage::NONE,
        prune: false,
        decoded: Vec::new(),
        component_breakdown: Vec::new(),
        novelty_detail: NoveltyDetail {
            count_novelty: novelty(0.0),
            cell_key: CellKey::new(0),
            cell_count: 0,
            visual_novelty: novelty(0.0),
            phash: 0,
            phash_min_hamming: 64,
            rnd_error: finite(0.0),
            knn_distance: finite(0.0),
        },
    }
}

fn decoded_values(state: GridState) -> Vec<DecodedValue> {
    vec![
        DecodedValue::Number(finite(f64::from(state.room.id()))),
        DecodedValue::Number(finite(f64::from(state.x))),
        DecodedValue::Number(finite(f64::from(state.y))),
        DecodedValue::Number(finite(f64::from(state.keys))),
        if state.boss_hp == 0 {
            DecodedValue::Invalid
        } else {
            DecodedValue::Number(finite(f64::from(state.boss_hp)))
        },
    ]
}

fn component_breakdown(state: GridState) -> Vec<ComponentScore> {
    vec![
        component("stage/start", state.room == Room::Start, 10.0),
        component("stage/key", state.has_key(), 25.0),
        component("stage/boss", state.room == Room::Boss, 50.0),
        component("shape/x-progress", true, f64::from(state.x)),
        component("penalty/prune", prune(state), 0.0),
    ]
}

fn component(name: &str, unlocked: bool, value: f64) -> ComponentScore {
    ComponentScore {
        name: name.to_owned(),
        value: finite(if unlocked { value } else { 0.0 }),
        unlocked,
    }
}

fn progress_score(state: GridState) -> Score {
    let value = f64::from(state.room.id()) * 100.0
        + f64::from(state.x) * 10.0
        + f64::from(GRID_HEIGHT - 1 - state.y)
        + if state.has_key() { 25.0 } else { 0.0 }
        + f64::from(BOSS_MAX_HP - state.boss_hp) * 40.0
        + if state.goal_reached() { 1_000.0 } else { 0.0 };
    score(value)
}

fn stage(state: GridState) -> Stage {
    if state.goal_reached() {
        Stage::new(4)
    } else if state.room == Room::Boss {
        Stage::new(3)
    } else if state.has_key() {
        Stage::new(2)
    } else if state.room == Room::Start {
        Stage::new(1)
    } else {
        Stage::NONE
    }
}

fn prune(state: GridState) -> bool {
    state.room == Room::Start && state.x == 0 && state.y == 0
}

fn visual_novelty_hash(state: GridState, framebuffer: Option<&Framebuffer>) -> u64 {
    if framebuffer.is_none() {
        return 0;
    }
    let digest = blake3::hash(state.state_hash().as_bytes());
    u64::from_le_bytes(
        digest.as_bytes()[..8]
            .try_into()
            .expect("slice has 8 bytes"),
    )
}

fn layout_len(layout: &orch_clients::scorer::CompiledLayout) -> ClientResult<u32> {
    layout.ranges.iter().try_fold(0u32, |sum, range| {
        sum.checked_add(range.len).ok_or_else(|| {
            ClientError::new(
                ClientErrorKind::InvalidRequest,
                "feature layout length overflow",
            )
        })
    })
}

fn layout_signature(layout: &orch_clients::scorer::CompiledLayout) -> Digest32 {
    Digest32::new(
        *blake3::hash(&postcard::to_allocvec(layout).expect("layout serializes")).as_bytes(),
    )
}

fn hash_artifact_source(domain: &[u8], source: &ArtifactSource) -> Digest32 {
    let mut hasher = blake3::Hasher::new();
    update_len_prefixed(&mut hasher, domain);
    match source {
        ArtifactSource::InlineYaml(bytes) => {
            update_len_prefixed(&mut hasher, b"inline");
            update_len_prefixed(&mut hasher, bytes);
        }
        ArtifactSource::ArtifactRef(reference) => {
            update_len_prefixed(&mut hasher, b"artifact_ref");
            update_len_prefixed(&mut hasher, reference.as_bytes());
        }
    }
    Digest32::new(*hasher.finalize().as_bytes())
}

fn archive_ref(hash: Digest32) -> String {
    let mut out = String::from("scar:blake3:");
    for byte in hash.as_bytes() {
        out.push(hex_char(byte >> 4));
        out.push(hex_char(byte & 0x0f));
    }
    out
}

fn hex_char(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        10..=15 => (b'a' + (nibble - 10)) as char,
        _ => unreachable!("nibble is masked to 4 bits"),
    }
}

fn update_len_prefixed(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn finite(value: f64) -> FiniteF64 {
    FiniteF64::new(value).expect("fake scorer only creates finite values")
}

fn score(value: f64) -> Score {
    Score::new(value).expect("fake scorer only creates finite scores")
}

fn novelty(value: f64) -> Novelty {
    Novelty::new(value).expect("fake scorer only creates finite novelty")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::KEY_BOSS_DOOR;
    use orch_clients::scorer::{
        CommittedState, CompiledLayout, ExtractRange, FramebufferMeta, LoadFeatureMapRequest,
        LoadScoringProgramRequest, PixelFormat, StateInput,
    };

    #[test]
    fn scorer_batch_id_dedup_replays_recorded_response_without_double_insert() {
        let mut scorer = loaded_scorer();
        let request = score_request("b1", [grid_state(0, 2, Room::Start, 0, BOSS_MAX_HP)]);

        let first = scorer.score_batch(request.clone()).expect("first score");
        let retry = scorer.score_batch(request).expect("retry score");
        let second_batch = scorer
            .score_batch(score_request(
                "b2",
                [grid_state(0, 2, Room::Start, 0, BOSS_MAX_HP)],
            ))
            .expect("second score");

        assert_eq!(first, retry);
        assert_eq!(first.archive_seq, 1);
        assert_eq!(scorer.archive_seq("exp-a"), Some(2));
        assert!(!first.results[0].duplicate);
        assert!(second_batch.results[0].duplicate);
        assert_eq!(second_batch.results[0].novelty_detail.cell_count, 1);
    }

    #[test]
    fn scorer_duplicate_detection_cell_counts_stage_names_and_prune() {
        let mut scorer = loaded_scorer();
        let program = scorer
            .load_scoring_program(LoadScoringProgramRequest {
                experiment_id: "exp-a".to_owned(),
                source: ArtifactSource::InlineYaml(b"program: fake-v1".to_vec()),
            })
            .expect("reload identical program");
        let response = scorer
            .score_batch(score_request(
                "b1",
                [
                    grid_state(0, 0, Room::Start, 0, BOSS_MAX_HP),
                    grid_state(2, 2, Room::KeyVault, KEY_BOSS_DOOR, BOSS_MAX_HP),
                    grid_state(4, 0, Room::Boss, KEY_BOSS_DOOR, 0),
                ],
            ))
            .expect("score batch");

        assert_eq!(program.stage_names, ["start", "key", "boss", "credits"]);
        assert_eq!(response.results[0].stage, Stage::new(1));
        assert!(response.results[0].prune);
        assert_eq!(response.results[1].stage, Stage::new(2));
        assert_eq!(response.results[2].stage, Stage::new(4));
        assert!(response.results[2].goal_hit);
        assert_eq!(
            response.results[1].decoded[3],
            DecodedValue::Number(finite(f64::from(KEY_BOSS_DOOR)))
        );
        assert_eq!(
            response.results[2].decoded,
            vec![
                DecodedValue::Number(finite(f64::from(Room::Boss.id()))),
                DecodedValue::Number(finite(4.0)),
                DecodedValue::Number(finite(0.0)),
                DecodedValue::Number(finite(f64::from(KEY_BOSS_DOOR))),
                DecodedValue::Invalid,
            ],
            "decoded values follow [room, x, y, keys, boss_hp] capture order"
        );
        assert_eq!(
            response.results[0].component_breakdown[0].name,
            "stage/start"
        );
    }

    #[test]
    fn scorer_item_errors_cover_feature_and_frame_guards() {
        let mut scorer = loaded_scorer();
        let mut bad_len = state_input("bad-len", grid_state(0, 2, Room::Start, 0, BOSS_MAX_HP));
        bad_len.feature_bytes.pop();
        let mut bad_decode =
            state_input("bad-decode", grid_state(0, 2, Room::Start, 0, BOSS_MAX_HP));
        bad_decode.feature_bytes[0] = 99;
        let mut bad_frame = state_input("bad-frame", grid_state(0, 2, Room::Start, 0, BOSS_MAX_HP));
        bad_frame.framebuffer = Some(Framebuffer::Raw(vec![0; 4]));
        bad_frame.fb_meta = Some(FramebufferMeta {
            width: 2,
            height: 1,
            format: PixelFormat::Gray8,
            uncompressed_len: 2,
        });

        let response = scorer
            .score_batch(ScoreBatchRequest {
                experiment_id: "exp-a".to_owned(),
                states: vec![bad_len, bad_decode, bad_frame],
                archive_update: ArchiveUpdateMode::ScoreAndInsert,
                client_batch_id: "b-errors".to_owned(),
                return_decoded: true,
            })
            .expect("error batch");

        assert_eq!(
            response.results[0].error.as_ref().map(|error| error.kind),
            Some(ItemErrorKind::FeatureLenMismatch)
        );
        assert_eq!(
            response.results[1].error.as_ref().map(|error| error.kind),
            Some(ItemErrorKind::DecodeFailed)
        );
        assert_eq!(
            response.results[2].error.as_ref().map(|error| error.kind),
            Some(ItemErrorKind::FbMetaMismatch)
        );
        assert!(response
            .results
            .iter()
            .all(|result| result.decoded.is_empty()));
    }

    #[test]
    fn scorer_archive_checkpoint_restore_and_replay_repairs_seen_and_cells() {
        let mut scorer = loaded_scorer();
        let state_a = grid_state(0, 2, Room::Start, 0, BOSS_MAX_HP);
        let state_b = grid_state(2, 2, Room::KeyVault, KEY_BOSS_DOOR, BOSS_MAX_HP);
        let state_b_hash = state_b.state_hash();
        let state_b_cell = state_b.cell_key();

        scorer
            .score_batch(score_request("b1", [state_a]))
            .expect("score a");
        let checkpoint = scorer
            .checkpoint_archive(CheckpointArchiveRequest {
                experiment_id: "exp-a".to_owned(),
                checkpoint_id: "cp1".to_owned(),
            })
            .expect("checkpoint");
        assert_eq!(checkpoint.cell_count, 1);
        assert_eq!(checkpoint.phash_count, 0);
        scorer
            .score_batch(score_request("b2", [state_b]))
            .expect("score b");
        let restored = scorer
            .restore_archive(RestoreArchiveRequest {
                experiment_id: "exp-a".to_owned(),
                checkpoint_id: "cp1".to_owned(),
                archive_ref: checkpoint.archive_ref,
            })
            .expect("restore");
        let replay = scorer
            .replay_commits(ReplayCommitsRequest {
                experiment_id: "exp-a".to_owned(),
                states: vec![
                    CommittedState {
                        state_hash: state_b_hash,
                        cell_key: state_b_cell,
                    },
                    CommittedState {
                        state_hash: state_b_hash,
                        cell_key: state_b_cell,
                    },
                ],
            })
            .expect("replay");
        let after_replay = scorer
            .score_batch(score_request("b3", [state_b]))
            .expect("score b after replay");

        assert_eq!(restored.archive_seq, checkpoint.archive_seq);
        assert_eq!(restored.cell_count, 1);
        assert_eq!(restored.phash_count, 0);
        assert_eq!(replay.applied, 1);
        assert_eq!(replay.skipped, 1);
        assert_eq!(
            scorer.archive_seq("exp-a"),
            Some(checkpoint.archive_seq + 1)
        );
        assert!(after_replay.results[0].duplicate);
        assert_eq!(after_replay.results[0].novelty_detail.cell_count, 1);
    }

    #[test]
    fn scorer_rebin_resets_cells_but_preserves_seen_hashes() {
        let mut scorer = loaded_scorer();
        let state = grid_state(2, 2, Room::KeyVault, KEY_BOSS_DOOR, BOSS_MAX_HP);
        scorer
            .score_batch(score_request("b1", [state]))
            .expect("initial score");

        let rebin = scorer
            .load_feature_map(LoadFeatureMapRequest {
                experiment_id: "exp-a".to_owned(),
                source: ArtifactSource::InlineYaml(b"feature-map: fake-v2".to_vec()),
                layout: sample_layout(),
                frame: Some(sample_frame()),
                rebin: true,
            })
            .expect("rebin feature map");
        let after_rebin = scorer
            .score_batch(score_request("b2", [state]))
            .expect("score after rebin");

        assert_eq!(rebin.warnings, ["rebin reset fake scorer cell counts"]);
        assert!(after_rebin.results[0].duplicate);
        assert_eq!(after_rebin.results[0].novelty_detail.cell_count, 0);
    }

    #[test]
    fn scorer_restore_rejects_loaded_artifact_binding_mismatch_without_mutation() {
        let mut scorer = loaded_scorer();
        let state = grid_state(0, 2, Room::Start, 0, BOSS_MAX_HP);
        scorer
            .score_batch(score_request("b1", [state]))
            .expect("initial score");
        let checkpoint = scorer
            .checkpoint_archive(CheckpointArchiveRequest {
                experiment_id: "exp-a".to_owned(),
                checkpoint_id: "cp1".to_owned(),
            })
            .expect("checkpoint");
        scorer
            .load_feature_map(LoadFeatureMapRequest {
                experiment_id: "exp-a".to_owned(),
                source: ArtifactSource::InlineYaml(b"feature-map: fake-v2".to_vec()),
                layout: sample_layout(),
                frame: Some(sample_frame()),
                rebin: true,
            })
            .expect("rebin feature map");

        let error = scorer
            .restore_archive(RestoreArchiveRequest {
                experiment_id: "exp-a".to_owned(),
                checkpoint_id: "cp1".to_owned(),
                archive_ref: checkpoint.archive_ref,
            })
            .expect_err("restore should reject mismatched loaded bindings");

        assert_eq!(error.kind(), ClientErrorKind::FailedPrecondition);
        assert_eq!(scorer.archive_seq("exp-a"), Some(1));
    }

    fn loaded_scorer() -> FakeScorer {
        let mut scorer = FakeScorer::new();
        scorer
            .load_feature_map(LoadFeatureMapRequest {
                experiment_id: "exp-a".to_owned(),
                source: ArtifactSource::InlineYaml(b"feature-map: fake-v1".to_vec()),
                layout: sample_layout(),
                frame: Some(sample_frame()),
                rebin: false,
            })
            .expect("load feature map");
        scorer
            .load_scoring_program(LoadScoringProgramRequest {
                experiment_id: "exp-a".to_owned(),
                source: ArtifactSource::InlineYaml(b"program: fake-v1".to_vec()),
            })
            .expect("load scoring program");
        scorer
    }

    fn score_request<const N: usize>(
        client_batch_id: &str,
        states: [GridState; N],
    ) -> ScoreBatchRequest {
        ScoreBatchRequest {
            experiment_id: "exp-a".to_owned(),
            states: states
                .into_iter()
                .enumerate()
                .map(|(index, state)| state_input(&format!("node-{index}"), state))
                .collect(),
            archive_update: ArchiveUpdateMode::ScoreAndInsert,
            client_batch_id: client_batch_id.to_owned(),
            return_decoded: true,
        }
    }

    fn state_input(node_ref: &str, state: GridState) -> StateInput {
        StateInput {
            node_ref: node_ref.to_owned(),
            feature_bytes: encode_grid_features(state),
            framebuffer: None,
            fb_meta: None,
        }
    }

    fn grid_state(x: u8, y: u8, room: Room, keys: u8, boss_hp: u8) -> GridState {
        GridState {
            x,
            y,
            room,
            keys,
            boss_hp,
        }
    }

    fn sample_layout() -> CompiledLayout {
        CompiledLayout {
            ranges: vec![ExtractRange {
                region: "grid".to_owned(),
                layout_version: 1,
                offset: 0,
                len: GRID_FEATURE_BYTES_LEN,
            }],
        }
    }

    fn sample_frame() -> FrameSpec {
        FrameSpec {
            width: 1,
            height: 1,
            stride: 4,
            format: PixelFormat::Xrgb8888,
        }
    }
}
