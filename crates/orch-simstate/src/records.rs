//! The journal record vocabulary (plan D-T2).
//!
//! One variant per **mutating** client-trait method across the four service
//! boundaries. The selection rule is mechanical and exact: every `&mut self`
//! trait method gets a variant; `&self` methods are never journaled (that
//! also excludes the SlotView drain task's `list_slots` / `watch_slots` /
//! `worker_info`, which fire on a real-time timer at nondeterministic
//! instants). Errored ops are journaled too — e.g. the hypervisor's `run()`
//! mutates slot state and pushes watch events even on the error path.
//! Synth's `propose_bursts` / `mine_macros` are `&mut` but pure — journaled
//! by the rule, harmlessly.

use orch_clients::hypervisor::{
    CreateVmRequest, DestroyVmRequest, ForkRequest, InjectInputsRequest, RestoreSnapshotRequest,
    RunRequest, TakeSnapshotRequest,
};
use orch_clients::input_synth::{LoadMacroPackRequest, MineMacrosRequest, ProposeBurstsRequest};
use orch_clients::scorer::{
    CheckpointArchiveRequest, LoadFeatureMapRequest, LoadScoringProgramRequest,
    ReplayCommitsRequest, RestoreArchiveRequest, ScoreBatchRequest,
};
use orch_clients::snapshot_store::{
    CreateNodeRequest, DeleteMetadataRequest, PruneSubtreeRequest, PutMetadataRequest,
    UpdateNodesRequest,
};
use serde::{Deserialize, Serialize};

// Variant sizes track the request DTOs they carry; boxing per-variant would
// complicate replay pattern-matching for zero benefit in a test-support
// crate.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum JournalRecord {
    /// Always the first frame; refuse to load any other version.
    Header {
        version: u32,
    },
    /// Advisory response-digest pairing for the op with the same `op_id`
    /// (never "the next frame" — ops from different services interleave).
    Applied {
        op_id: u64,
        digest: u64,
    },
    /// The reload path's session reclaim (D-T4), journaled so the *next*
    /// incarnation's replay reproduces it in order.
    ReclaimSession {
        op_id: u64,
    },

    // Hypervisor (&mut self)
    HvCreateVm {
        op_id: u64,
        request: CreateVmRequest,
    },
    HvRestoreSnapshot {
        op_id: u64,
        request: RestoreSnapshotRequest,
    },
    HvFork {
        op_id: u64,
        request: ForkRequest,
    },
    HvInjectInputs {
        op_id: u64,
        request: InjectInputsRequest,
    },
    HvRun {
        op_id: u64,
        request: RunRequest,
    },
    HvTakeSnapshot {
        op_id: u64,
        request: TakeSnapshotRequest,
    },
    HvDestroyVm {
        op_id: u64,
        request: DestroyVmRequest,
    },

    // Snapshot store (&mut self)
    StCreateNode {
        op_id: u64,
        request: CreateNodeRequest,
    },
    StUpdateNodes {
        op_id: u64,
        request: UpdateNodesRequest,
    },
    StPutMetadata {
        op_id: u64,
        request: PutMetadataRequest,
    },
    StDeleteMetadata {
        op_id: u64,
        request: DeleteMetadataRequest,
    },
    StPruneSubtree {
        op_id: u64,
        request: PruneSubtreeRequest,
    },

    // Scorer (&mut self)
    ScLoadFeatureMap {
        op_id: u64,
        request: LoadFeatureMapRequest,
    },
    ScLoadScoringProgram {
        op_id: u64,
        request: LoadScoringProgramRequest,
    },
    ScScoreBatch {
        op_id: u64,
        request: ScoreBatchRequest,
    },
    ScCheckpointArchive {
        op_id: u64,
        request: CheckpointArchiveRequest,
    },
    ScRestoreArchive {
        op_id: u64,
        request: RestoreArchiveRequest,
    },
    ScReplayCommits {
        op_id: u64,
        request: ReplayCommitsRequest,
    },

    // Input synth (&mut self)
    SyLoadMacroPack {
        op_id: u64,
        request: LoadMacroPackRequest,
    },
    SyProposeBursts {
        op_id: u64,
        request: ProposeBurstsRequest,
    },
    SyMineMacros {
        op_id: u64,
        request: MineMacrosRequest,
    },
}

impl JournalRecord {
    /// The op id carried by every record except the header.
    #[must_use]
    pub fn op_id(&self) -> Option<u64> {
        match self {
            Self::Header { .. } => None,
            Self::Applied { op_id, .. }
            | Self::ReclaimSession { op_id }
            | Self::HvCreateVm { op_id, .. }
            | Self::HvRestoreSnapshot { op_id, .. }
            | Self::HvFork { op_id, .. }
            | Self::HvInjectInputs { op_id, .. }
            | Self::HvRun { op_id, .. }
            | Self::HvTakeSnapshot { op_id, .. }
            | Self::HvDestroyVm { op_id, .. }
            | Self::StCreateNode { op_id, .. }
            | Self::StUpdateNodes { op_id, .. }
            | Self::StPutMetadata { op_id, .. }
            | Self::StDeleteMetadata { op_id, .. }
            | Self::StPruneSubtree { op_id, .. }
            | Self::ScLoadFeatureMap { op_id, .. }
            | Self::ScLoadScoringProgram { op_id, .. }
            | Self::ScScoreBatch { op_id, .. }
            | Self::ScCheckpointArchive { op_id, .. }
            | Self::ScRestoreArchive { op_id, .. }
            | Self::ScReplayCommits { op_id, .. }
            | Self::SyLoadMacroPack { op_id, .. }
            | Self::SyProposeBursts { op_id, .. }
            | Self::SyMineMacros { op_id, .. } => Some(*op_id),
        }
    }
}
