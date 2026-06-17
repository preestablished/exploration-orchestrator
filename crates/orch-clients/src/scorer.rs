//! State scorer client boundary.
//!
//! Owner docs: `/home/infra-admin/.agents/projects/determinism/docs/state-scorer/API.md`
//! section 1 and
//! `/home/infra-admin/.agents/projects/determinism/docs/exploration-orchestrator/API.md`
//! section 5.
//!
//! This module mirrors feature-map loading, scoring program loading, batch
//! scoring, archive checkpoint/restore, replay repair, decoded values, and
//! re-bin semantics without choosing a transport implementation.

use orch_core::types::{CellKey, FiniteF64, Novelty, Score, Stage, StateHash};
use serde::{Deserialize, Serialize};

use crate::ClientResult;

pub trait StateScorerClient {
    fn load_feature_map(
        &mut self,
        request: LoadFeatureMapRequest,
    ) -> ClientResult<LoadFeatureMapResponse>;

    fn load_scoring_program(
        &mut self,
        request: LoadScoringProgramRequest,
    ) -> ClientResult<LoadScoringProgramResponse>;

    fn score_batch(&mut self, request: ScoreBatchRequest) -> ClientResult<ScoreBatchResponse>;

    fn checkpoint_archive(
        &mut self,
        request: CheckpointArchiveRequest,
    ) -> ClientResult<CheckpointArchiveResponse>;

    fn restore_archive(
        &mut self,
        request: RestoreArchiveRequest,
    ) -> ClientResult<RestoreArchiveResponse>;

    fn replay_commits(
        &mut self,
        request: ReplayCommitsRequest,
    ) -> ClientResult<ReplayCommitsResponse>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadFeatureMapRequest {
    pub experiment_id: String,
    pub source: ArtifactSource,
    pub layout: CompiledLayout,
    pub frame: Option<FrameSpec>,
    pub rebin: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadFeatureMapResponse {
    pub feature_map_hash: Digest32,
    pub field_count: u32,
    pub feature_bytes_len: u32,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadScoringProgramRequest {
    pub experiment_id: String,
    pub source: ArtifactSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadScoringProgramResponse {
    pub program_hash: Digest32,
    pub component_names: Vec<String>,
    pub goal_expr: String,
    pub warnings: Vec<String>,
    pub stage_names: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactSource {
    InlineYaml(Vec<u8>),
    ArtifactRef(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledLayout {
    pub ranges: Vec<ExtractRange>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractRange {
    pub region: String,
    pub layout_version: u32,
    pub offset: u64,
    pub len: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameSpec {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoreBatchRequest {
    pub experiment_id: String,
    pub states: Vec<StateInput>,
    pub archive_update: ArchiveUpdateMode,
    pub client_batch_id: String,
    pub return_decoded: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateInput {
    pub node_ref: String,
    pub feature_bytes: Vec<u8>,
    pub framebuffer: Option<Framebuffer>,
    pub fb_meta: Option<FramebufferMeta>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Framebuffer {
    Lz4(Vec<u8>),
    Raw(Vec<u8>),
    BlobRef(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FramebufferMeta {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub uncompressed_len: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoreBatchResponse {
    pub client_batch_id: String,
    pub archive_seq: u64,
    pub results: Vec<ScoreResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoreResult {
    pub node_ref: String,
    pub error: Option<ItemError>,
    pub progress_score: Score,
    pub novelty_score: Novelty,
    pub state_hash: StateHash,
    pub goal_hit: bool,
    pub duplicate: bool,
    pub stage: Stage,
    pub prune: bool,
    pub decoded: Vec<DecodedValue>,
    pub component_breakdown: Vec<ComponentScore>,
    pub novelty_detail: NoveltyDetail,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DecodedValue {
    Number(FiniteF64),
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComponentScore {
    pub name: String,
    pub value: FiniteF64,
    pub unlocked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoveltyDetail {
    pub count_novelty: Novelty,
    pub cell_key: CellKey,
    pub cell_count: u32,
    pub visual_novelty: Novelty,
    pub phash: u64,
    pub phash_min_hamming: u32,
    pub rnd_error: FiniteF64,
    pub knn_distance: FiniteF64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemError {
    pub kind: ItemErrorKind,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointArchiveRequest {
    pub experiment_id: String,
    pub checkpoint_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointArchiveResponse {
    pub archive_ref: String,
    pub archive_hash: Digest32,
    pub archive_seq: u64,
    pub cell_count: u64,
    pub phash_count: u64,
    pub embedding_count: u64,
    pub blob_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreArchiveRequest {
    pub experiment_id: String,
    pub checkpoint_id: String,
    pub archive_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreArchiveResponse {
    pub archive_seq: u64,
    pub cell_count: u64,
    pub phash_count: u64,
    pub embedding_count: u64,
    pub bound_feature_map_hash: Digest32,
    pub bound_scoring_program_hash: Digest32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayCommitsRequest {
    pub experiment_id: String,
    pub states: Vec<CommittedState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommittedState {
    pub state_hash: StateHash,
    pub cell_key: CellKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayCommitsResponse {
    pub applied: u64,
    pub skipped: u64,
}

pub const DIGEST32_LEN: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Digest32(pub [u8; DIGEST32_LEN]);

impl Digest32 {
    #[must_use]
    pub const fn new(bytes: [u8; DIGEST32_LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; DIGEST32_LEN] {
        &self.0
    }

    #[must_use]
    pub const fn into_bytes(self) -> [u8; DIGEST32_LEN] {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArchiveUpdateMode {
    ScoreAndInsert,
    ScoreOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PixelFormat {
    Rgb888,
    Rgb555Le,
    Gray8,
    Xrgb8888,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemErrorKind {
    FeatureLenMismatch,
    DecodeFailed,
    FbMetaMismatch,
    FbDecompressFailed,
    FbRefUnsupported,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEATURE_HASH: Digest32 = Digest32::new([0x11; DIGEST32_LEN]);
    const PROGRAM_HASH: Digest32 = Digest32::new([0x22; DIGEST32_LEN]);
    const ARCHIVE_HASH: Digest32 = Digest32::new([0x33; DIGEST32_LEN]);
    const STATE_A: StateHash = StateHash::new([0xA5; 32]);
    const STATE_B: StateHash = StateHash::new([0x5A; 32]);

    #[test]
    fn scorer_feature_map_request_carries_rebin_layout_and_frame_spec() {
        let request = LoadFeatureMapRequest {
            experiment_id: "exp-a".to_owned(),
            source: ArtifactSource::InlineYaml(b"meta:\n  version: 2\n".to_vec()),
            layout: sample_layout(),
            frame: Some(FrameSpec {
                width: 256,
                height: 224,
                stride: 1024,
                format: PixelFormat::Xrgb8888,
            }),
            rebin: true,
        };
        let response = LoadFeatureMapResponse {
            feature_map_hash: FEATURE_HASH,
            field_count: 12,
            feature_bytes_len: 64,
            warnings: vec!["unknown semantics treated as opaque".to_owned()],
        };

        assert!(request.rebin);
        assert_eq!(request.layout.ranges[0].region, "wram");
        assert_eq!(request.layout.ranges[0].len, 64);
        assert_eq!(
            request.frame.expect("frame spec").format,
            PixelFormat::Xrgb8888
        );
        assert_eq!(response.feature_map_hash, FEATURE_HASH);
        assert_eq!(response.feature_bytes_len, 64);
    }

    #[test]
    fn scorer_program_response_carries_components_goal_and_stage_names() {
        let request = LoadScoringProgramRequest {
            experiment_id: "exp-a".to_owned(),
            source: ArtifactSource::ArtifactRef("artifact://score/demo-v1".to_owned()),
        };
        let response = LoadScoringProgramResponse {
            program_hash: PROGRAM_HASH,
            component_names: vec![
                "stage/start".to_owned(),
                "shape/x-progress".to_owned(),
                "penalty/spike".to_owned(),
            ],
            goal_expr: "credits == 1".to_owned(),
            warnings: Vec::new(),
            stage_names: vec!["start".to_owned(), "boss".to_owned()],
        };

        assert!(matches!(request.source, ArtifactSource::ArtifactRef(_)));
        assert_eq!(response.program_hash, PROGRAM_HASH);
        assert_eq!(response.component_names[0], "stage/start");
        assert_eq!(response.stage_names, ["start", "boss"]);
    }

    #[test]
    fn scorer_score_batch_preserves_client_batch_id_and_result_order() {
        let request = ScoreBatchRequest {
            experiment_id: "exp-a".to_owned(),
            states: vec![sample_state_input("42/0"), sample_state_input("42/1")],
            archive_update: ArchiveUpdateMode::ScoreAndInsert,
            client_batch_id: "b42".to_owned(),
            return_decoded: true,
        };
        let response = ScoreBatchResponse {
            client_batch_id: "b42".to_owned(),
            archive_seq: 9,
            results: vec![
                sample_score_result("42/0", STATE_A, CellKey::new(8), false),
                sample_score_result("42/1", STATE_B, CellKey::new(13), true),
            ],
        };

        assert_eq!(request.client_batch_id, response.client_batch_id);
        assert_eq!(response.archive_seq, 9);
        assert_eq!(response.results[0].node_ref, request.states[0].node_ref);
        assert_eq!(response.results[1].node_ref, request.states[1].node_ref);
        assert!(response.results[1].duplicate);
    }

    #[test]
    fn scorer_result_carries_decoded_values_and_novelty_detail() {
        let result = sample_score_result("42/0", STATE_A, CellKey::new(8), false);

        assert_eq!(
            result.decoded,
            vec![DecodedValue::Number(finite(12.0)), DecodedValue::Invalid]
        );
        assert_eq!(result.novelty_detail.cell_key, CellKey::new(8));
        assert_eq!(result.novelty_detail.cell_count, 3);
        assert_eq!(result.novelty_detail.phash_min_hamming, 64);
        assert_eq!(result.component_breakdown[0].name, "stage/start");
        assert!(result.component_breakdown[0].unlocked);
    }

    #[test]
    fn scorer_archive_checkpoint_restore_and_replay_shapes_match_lockstep() {
        let checkpoint = CheckpointArchiveResponse {
            archive_ref: "scar:blake3:333333".to_owned(),
            archive_hash: ARCHIVE_HASH,
            archive_seq: 10,
            cell_count: 20,
            phash_count: 5,
            embedding_count: 2,
            blob_bytes: 4096,
        };
        let restore = RestoreArchiveResponse {
            archive_seq: checkpoint.archive_seq,
            cell_count: checkpoint.cell_count,
            phash_count: checkpoint.phash_count,
            embedding_count: checkpoint.embedding_count,
            bound_feature_map_hash: FEATURE_HASH,
            bound_scoring_program_hash: PROGRAM_HASH,
        };
        let replay_request = ReplayCommitsRequest {
            experiment_id: "exp-a".to_owned(),
            states: vec![
                CommittedState {
                    state_hash: STATE_A,
                    cell_key: CellKey::new(8),
                },
                CommittedState {
                    state_hash: STATE_B,
                    cell_key: CellKey::new(13),
                },
            ],
        };
        let replay_response = ReplayCommitsResponse {
            applied: 1,
            skipped: 1,
        };

        assert_eq!(checkpoint.archive_ref, "scar:blake3:333333");
        assert_eq!(restore.archive_seq, 10);
        assert_eq!(restore.bound_feature_map_hash, FEATURE_HASH);
        assert_eq!(restore.bound_scoring_program_hash, PROGRAM_HASH);
        assert_eq!(replay_request.states[0].state_hash, STATE_A);
        assert_eq!(replay_request.states[1].cell_key, CellKey::new(13));
        assert_eq!(replay_response.applied + replay_response.skipped, 2);
    }

    #[test]
    fn scorer_score_batch_dtos_round_trip_with_postcard() {
        let response = ScoreBatchResponse {
            client_batch_id: "b42".to_owned(),
            archive_seq: 9,
            results: vec![sample_score_result("42/0", STATE_A, CellKey::new(8), false)],
        };
        let encoded = postcard::to_allocvec(&response).expect("serialize score response");
        let decoded: ScoreBatchResponse =
            postcard::from_bytes(&encoded).expect("deserialize score response");
        let encoded_again = postcard::to_allocvec(&decoded).expect("reserialize");

        assert_eq!(decoded, response);
        assert_eq!(encoded_again, encoded);
    }

    fn sample_layout() -> CompiledLayout {
        CompiledLayout {
            ranges: vec![ExtractRange {
                region: "wram".to_owned(),
                layout_version: 1,
                offset: 0x1000,
                len: 64,
            }],
        }
    }

    fn sample_state_input(node_ref: &str) -> StateInput {
        StateInput {
            node_ref: node_ref.to_owned(),
            feature_bytes: vec![1, 2, 3, 4],
            framebuffer: Some(Framebuffer::Lz4(vec![0xAA, 0xBB])),
            fb_meta: Some(FramebufferMeta {
                width: 256,
                height: 224,
                format: PixelFormat::Xrgb8888,
                uncompressed_len: 229_376,
            }),
        }
    }

    fn sample_score_result(
        node_ref: &str,
        state_hash: StateHash,
        cell_key: CellKey,
        duplicate: bool,
    ) -> ScoreResult {
        ScoreResult {
            node_ref: node_ref.to_owned(),
            error: None,
            progress_score: score(12.0),
            novelty_score: novelty(0.75),
            state_hash,
            goal_hit: false,
            duplicate,
            stage: Stage::new(2),
            prune: false,
            decoded: vec![DecodedValue::Number(finite(12.0)), DecodedValue::Invalid],
            component_breakdown: vec![ComponentScore {
                name: "stage/start".to_owned(),
                value: finite(10.0),
                unlocked: true,
            }],
            novelty_detail: NoveltyDetail {
                count_novelty: novelty(0.5),
                cell_key,
                cell_count: 3,
                visual_novelty: novelty(1.0),
                phash: 0,
                phash_min_hamming: 64,
                rnd_error: finite(0.0),
                knn_distance: finite(0.0),
            },
        }
    }

    fn finite(value: f64) -> FiniteF64 {
        FiniteF64::new(value).expect("test value is finite")
    }

    fn score(value: f64) -> Score {
        Score::new(value).expect("test score is finite")
    }

    fn novelty(value: f64) -> Novelty {
        Novelty::new(value).expect("test novelty is finite")
    }
}
