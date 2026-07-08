//! Journaling wrappers over the four fakes, and the persistent fake world
//! (plan W2.2, D-T2/D-T4).
//!
//! [`Persistent<T>`] implements the same sync client traits as the fake it
//! wraps; with a journal it write-ahead-logs every mutating op, without one
//! it is a pure passthrough — the same concrete type serves both
//! `--state-dir` modes (the sched ports are RPITIT, not dyn-safe).
//!
//! Write path, all within the one `SyncAdapter`-held call: take the journal
//! mutex, assign the next op id, append the op frame (fsync) — apply to the
//! inner fake — append `Applied { op_id, digest }` (advisory, no fsync),
//! return. A SIGKILL between append and apply is indistinguishable from
//! "server executed, response lost"; replay treats the re-invoked result as
//! authoritative there.

use std::path::Path;
use std::sync::{Arc, Mutex};

use orch_clients::hypervisor::{
    CreateVmRequest, CreateVmResponse, DestroyVmRequest, DestroyVmResponse, ForkRequest,
    ForkResponse, GetWorkerInfoRequest, GetWorkerInfoResponse, HypervisorWorkerClient,
    InjectInputsRequest, InjectInputsResponse, ListSlotsRequest, ListSlotsResponse,
    RestoreSnapshotRequest, RestoreSnapshotResponse, RunRequest, RunResponse, TakeSnapshotRequest,
    TakeSnapshotResponse, WatchSlotsRequest, WatchSlotsResponse,
};
use orch_clients::input_synth::{
    HealthRequest, HealthResponse, InputSynthClient, LoadMacroPackRequest, LoadMacroPackResponse,
    MineMacrosRequest, MineMacrosResponse, ProposeBurstsRequest, ProposeBurstsResponse,
};
use orch_clients::scorer::{
    CheckpointArchiveRequest, CheckpointArchiveResponse, LoadFeatureMapRequest,
    LoadFeatureMapResponse, LoadScoringProgramRequest, LoadScoringProgramResponse,
    ReplayCommitsRequest, ReplayCommitsResponse, RestoreArchiveRequest, RestoreArchiveResponse,
    ScoreBatchRequest, ScoreBatchResponse, StateScorerClient,
};
use orch_clients::snapshot_store::{
    CreateNodeRequest, CreateNodeResponse, DeleteMetadataRequest, DeleteMetadataResponse,
    GetChildrenRequest, GetChildrenResponse, GetMetadataRequest, GetMetadataResponse,
    GetNodeRequest, GetNodeResponse, GetPathRequest, GetPathResponse, PruneSubtreeRequest,
    PruneSubtreeResponse, PutMetadataRequest, PutMetadataResponse, QueryNodesRequest,
    QueryNodesResponse, SnapshotStoreClient, UpdateNodesRequest, UpdateNodesResponse,
};
use orch_clients::ClientResult;
use orch_fakes::{
    grid::GridWorld, hypervisor::FakeHypervisor, observatory::FakeObservatory, scorer::FakeScorer,
    snapshot_store::InMemorySnapshotStore, synth::FakeSynth,
};
use orch_sched::ports::SyncAdapter;
use serde::Serialize;

use crate::journal::{truncated_blake3, Journal, LoadStats, RecordKind};
use crate::records::JournalRecord;

type SharedJournal = Arc<Mutex<Journal>>;

/// Response digest: truncated blake3 over the postcard encoding of the
/// result. `ClientError` has no serde derives and its kind is
/// `#[non_exhaustive]`, so errors digest via a local mirror
/// (kind-as-string + message) — see plan D-T2.
fn digest_response<R: Serialize>(result: &ClientResult<R>) -> u64 {
    #[derive(Serialize)]
    enum ResultMirror<'a, R: Serialize> {
        Ok(&'a R),
        Err { kind: &'a str, message: &'a str },
    }
    let mirror = match result {
        Ok(response) => ResultMirror::Ok(response),
        Err(error) => ResultMirror::Err {
            kind: error.kind().as_str(),
            message: error.message(),
        },
    };
    let bytes = postcard::to_allocvec(&mirror).expect("responses are postcard-serializable");
    truncated_blake3(&bytes)
}

fn metadata_record_kind(request: &PutMetadataRequest) -> RecordKind {
    let key = request.key.as_str();
    if key.starts_with("orch/wal/") {
        RecordKind::WalAppend
    } else if key.starts_with("orch/ckpt/") {
        RecordKind::CkptPut
    } else {
        RecordKind::Other
    }
}

/// Journaling wrapper over one fake service. `journal: None` is a pure
/// passthrough (journal-less `--state-dir`-free mode; zero behavior change).
pub struct Persistent<T> {
    inner: T,
    journal: Option<SharedJournal>,
}

impl<T> Persistent<T> {
    #[must_use]
    pub fn new(inner: T, journal: Option<SharedJournal>) -> Self {
        Self { inner, journal }
    }

    /// The wrapped fake, for comparators and test inspection.
    #[must_use]
    pub fn inner(&self) -> &T {
        &self.inner
    }
}

/// Write-ahead journal one mutating op, apply it, digest the response.
macro_rules! journaled {
    ($self:ident, $method:ident, $request:ident, $variant:ident, $kind:expr) => {{
        let Some(journal) = $self.journal.clone() else {
            return $self.inner.$method($request);
        };
        let op_id = journal.lock().expect("journal mutex").append_op(
            |op_id| JournalRecord::$variant {
                op_id,
                request: $request.clone(),
            },
            $kind,
        );
        let response = $self.inner.$method($request);
        let digest = digest_response(&response);
        journal
            .lock()
            .expect("journal mutex")
            .append_advisory(&JournalRecord::Applied { op_id, digest });
        response
    }};
}

impl<T: HypervisorWorkerClient> HypervisorWorkerClient for Persistent<T> {
    fn create_vm(&mut self, request: CreateVmRequest) -> ClientResult<CreateVmResponse> {
        journaled!(self, create_vm, request, HvCreateVm, RecordKind::Other)
    }

    fn restore_snapshot(
        &mut self,
        request: RestoreSnapshotRequest,
    ) -> ClientResult<RestoreSnapshotResponse> {
        journaled!(
            self,
            restore_snapshot,
            request,
            HvRestoreSnapshot,
            RecordKind::Other
        )
    }

    fn fork(&mut self, request: ForkRequest) -> ClientResult<ForkResponse> {
        journaled!(self, fork, request, HvFork, RecordKind::Other)
    }

    fn inject_inputs(
        &mut self,
        request: InjectInputsRequest,
    ) -> ClientResult<InjectInputsResponse> {
        journaled!(
            self,
            inject_inputs,
            request,
            HvInjectInputs,
            RecordKind::Other
        )
    }

    fn run(&mut self, request: RunRequest) -> ClientResult<RunResponse> {
        journaled!(self, run, request, HvRun, RecordKind::Other)
    }

    fn take_snapshot(
        &mut self,
        request: TakeSnapshotRequest,
    ) -> ClientResult<TakeSnapshotResponse> {
        journaled!(
            self,
            take_snapshot,
            request,
            HvTakeSnapshot,
            RecordKind::Other
        )
    }

    fn destroy_vm(&mut self, request: DestroyVmRequest) -> ClientResult<DestroyVmResponse> {
        journaled!(self, destroy_vm, request, HvDestroyVm, RecordKind::Other)
    }

    fn list_slots(&self, request: ListSlotsRequest) -> ClientResult<ListSlotsResponse> {
        self.inner.list_slots(request)
    }

    fn watch_slots(&self, request: WatchSlotsRequest) -> ClientResult<WatchSlotsResponse> {
        self.inner.watch_slots(request)
    }

    fn worker_info(&self, request: GetWorkerInfoRequest) -> ClientResult<GetWorkerInfoResponse> {
        self.inner.worker_info(request)
    }
}

impl<T: SnapshotStoreClient> SnapshotStoreClient for Persistent<T> {
    fn create_node(&mut self, request: CreateNodeRequest) -> ClientResult<CreateNodeResponse> {
        journaled!(self, create_node, request, StCreateNode, RecordKind::Other)
    }

    fn update_nodes(&mut self, request: UpdateNodesRequest) -> ClientResult<UpdateNodesResponse> {
        journaled!(
            self,
            update_nodes,
            request,
            StUpdateNodes,
            RecordKind::Other
        )
    }

    fn get_node(&self, request: GetNodeRequest) -> ClientResult<GetNodeResponse> {
        self.inner.get_node(request)
    }

    fn get_children(&self, request: GetChildrenRequest) -> ClientResult<GetChildrenResponse> {
        self.inner.get_children(request)
    }

    fn get_path(&self, request: GetPathRequest) -> ClientResult<GetPathResponse> {
        self.inner.get_path(request)
    }

    fn query_nodes(&self, request: QueryNodesRequest) -> ClientResult<QueryNodesResponse> {
        self.inner.query_nodes(request)
    }

    fn put_metadata(&mut self, request: PutMetadataRequest) -> ClientResult<PutMetadataResponse> {
        let kind = metadata_record_kind(&request);
        journaled!(self, put_metadata, request, StPutMetadata, kind)
    }

    fn get_metadata(&self, request: GetMetadataRequest) -> ClientResult<GetMetadataResponse> {
        self.inner.get_metadata(request)
    }

    fn delete_metadata(
        &mut self,
        request: DeleteMetadataRequest,
    ) -> ClientResult<DeleteMetadataResponse> {
        journaled!(
            self,
            delete_metadata,
            request,
            StDeleteMetadata,
            RecordKind::Other
        )
    }

    fn prune_subtree(
        &mut self,
        request: PruneSubtreeRequest,
    ) -> ClientResult<PruneSubtreeResponse> {
        journaled!(
            self,
            prune_subtree,
            request,
            StPruneSubtree,
            RecordKind::Other
        )
    }
}

impl<T: StateScorerClient> StateScorerClient for Persistent<T> {
    fn load_feature_map(
        &mut self,
        request: LoadFeatureMapRequest,
    ) -> ClientResult<LoadFeatureMapResponse> {
        journaled!(
            self,
            load_feature_map,
            request,
            ScLoadFeatureMap,
            RecordKind::Other
        )
    }

    fn load_scoring_program(
        &mut self,
        request: LoadScoringProgramRequest,
    ) -> ClientResult<LoadScoringProgramResponse> {
        journaled!(
            self,
            load_scoring_program,
            request,
            ScLoadScoringProgram,
            RecordKind::Other
        )
    }

    fn score_batch(&mut self, request: ScoreBatchRequest) -> ClientResult<ScoreBatchResponse> {
        journaled!(self, score_batch, request, ScScoreBatch, RecordKind::Other)
    }

    fn checkpoint_archive(
        &mut self,
        request: CheckpointArchiveRequest,
    ) -> ClientResult<CheckpointArchiveResponse> {
        journaled!(
            self,
            checkpoint_archive,
            request,
            ScCheckpointArchive,
            RecordKind::Other
        )
    }

    fn restore_archive(
        &mut self,
        request: RestoreArchiveRequest,
    ) -> ClientResult<RestoreArchiveResponse> {
        journaled!(
            self,
            restore_archive,
            request,
            ScRestoreArchive,
            RecordKind::Other
        )
    }

    fn replay_commits(
        &mut self,
        request: ReplayCommitsRequest,
    ) -> ClientResult<ReplayCommitsResponse> {
        journaled!(
            self,
            replay_commits,
            request,
            ScReplayCommits,
            RecordKind::Other
        )
    }
}

impl<T: InputSynthClient> InputSynthClient for Persistent<T> {
    fn load_macro_pack(
        &mut self,
        request: LoadMacroPackRequest,
    ) -> ClientResult<LoadMacroPackResponse> {
        journaled!(
            self,
            load_macro_pack,
            request,
            SyLoadMacroPack,
            RecordKind::Other
        )
    }

    fn health(&self, request: HealthRequest) -> ClientResult<HealthResponse> {
        self.inner.health(request)
    }

    fn propose_bursts(
        &mut self,
        request: ProposeBurstsRequest,
    ) -> ClientResult<ProposeBurstsResponse> {
        journaled!(
            self,
            propose_bursts,
            request,
            SyProposeBursts,
            RecordKind::Other
        )
    }

    fn mine_macros(&mut self, request: MineMacrosRequest) -> ClientResult<MineMacrosResponse> {
        journaled!(self, mine_macros, request, SyMineMacros, RecordKind::Other)
    }
}

/// Documented test-only replay mutations for the negative control (W2.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakMode {
    /// Bump one journaled `create_node` request's `progress_score` during
    /// replay: the replayed store diverges by one committed node's score.
    PerturbNode,
    /// Skip replaying scorer `score_batch` / `replay_commits` records:
    /// resume must fail the archive-lockstep guard
    /// (`REASON_ARCHIVE_SEQ_MISMATCH`).
    DropScorerReplay,
}

/// The four wrapped fakes plus their shared journal, before async adapters.
/// Unit tests drive this directly; the daemon wraps it into a
/// [`PersistentWorld`].
pub struct PersistentServices {
    pub hypervisor: Persistent<FakeHypervisor>,
    pub scorer: Persistent<FakeScorer>,
    pub store: Persistent<InMemorySnapshotStore>,
    pub synth: Persistent<FakeSynth>,
}

fn fresh_fakes() -> (FakeHypervisor, FakeScorer, InMemorySnapshotStore, FakeSynth) {
    // Must match `orchestratord`'s `--simulate` world (fault plans stay
    // disabled — the journal soundness invariant, see the crate docs).
    let world = GridWorld::three_room();
    (
        FakeHypervisor::with_world(world.clone()),
        FakeScorer::with_world(world),
        InMemorySnapshotStore::new(),
        FakeSynth::new(),
    )
}

impl PersistentServices {
    /// Journal-less passthrough world (no `--state-dir`).
    #[must_use]
    pub fn ephemeral() -> Self {
        let (hypervisor, scorer, store, synth) = fresh_fakes();
        Self {
            hypervisor: Persistent::new(hypervisor, None),
            scorer: Persistent::new(scorer, None),
            store: Persistent::new(store, None),
            synth: Persistent::new(synth, None),
        }
    }

    /// Fresh fakes over a brand-new journal in `dir`.
    pub fn create(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let journal = Arc::new(Mutex::new(Journal::create(dir)?));
        let (hypervisor, scorer, store, synth) = fresh_fakes();
        Ok(Self {
            hypervisor: Persistent::new(hypervisor, Some(Arc::clone(&journal))),
            scorer: Persistent::new(scorer, Some(Arc::clone(&journal))),
            store: Persistent::new(store, Some(Arc::clone(&journal))),
            synth: Persistent::new(synth, Some(journal)),
        })
    }

    /// Rebuilds the world by re-invoking every journaled op in order against
    /// fresh fakes, digest-checking each against its `Applied` frame (the
    /// tripwire for hidden nondeterminism in the fakes), then reclaims the
    /// dead incarnation's sessions (D-T4) and reopens the journal for
    /// appending. This is the **live resume** path — the returned world is
    /// journal-backed and drivable, and the reclaim is journaled so the next
    /// incarnation reproduces it.
    pub fn reload(dir: &Path) -> std::io::Result<(Self, LoadStats)> {
        Self::reload_inner(dir, None, false)
    }

    /// [`Self::reload`] with a deliberate divergence for the negative
    /// control (W2.5). Digest checks are skipped: the mutation exists to
    /// diverge, and the end-state comparator is what must catch it.
    pub fn reload_broken(dir: &Path, mode: BreakMode) -> std::io::Result<(Self, LoadStats)> {
        Self::reload_inner(dir, Some(mode), false)
    }

    /// Read-only reconstruction for end-state inspection (comparators). Unlike
    /// [`Self::reload`] it has **no on-disk side effect**: it neither reclaims
    /// sessions nor journals anything, so computing a fingerprint is
    /// idempotent and never grows a never-crashed control run's journal. The
    /// returned world is journal-less (do not drive it further).
    pub fn reload_readonly(dir: &Path) -> std::io::Result<(Self, LoadStats)> {
        Self::reload_inner(dir, None, true)
    }

    /// [`Self::reload_readonly`] re-applying a break mode — reads back a
    /// deliberately-broken state-dir for the negative control without
    /// mutating it (the broken run journaled its post-resume ops against
    /// perturbed state, so a plain read-only replay would trip the digest
    /// tripwire; re-applying the mutation reconstructs the divergent state).
    pub fn reload_broken_readonly(
        dir: &Path,
        mode: BreakMode,
    ) -> std::io::Result<(Self, LoadStats)> {
        Self::reload_inner(dir, Some(mode), true)
    }

    fn reload_inner(
        dir: &Path,
        break_mode: Option<BreakMode>,
        readonly: bool,
    ) -> std::io::Result<(Self, LoadStats)> {
        let (records, stats) = Journal::load(dir)?;
        let (mut hypervisor, mut scorer, mut store, mut synth) = fresh_fakes();

        let applied: std::collections::HashMap<u64, u64> = records
            .iter()
            .filter_map(|record| match record {
                JournalRecord::Applied { op_id, digest } => Some((*op_id, *digest)),
                _ => None,
            })
            .collect();
        let check = |op_id: u64, digest: u64| {
            if break_mode.is_some() {
                return;
            }
            // A missing Applied frame means the crash landed between append
            // and apply — the re-invoked result is authoritative.
            if let Some(expected) = applied.get(&op_id) {
                assert_eq!(
                    digest, *expected,
                    "replay digest mismatch at op {op_id}: hidden nondeterminism in the fakes"
                );
            }
        };

        let mut max_op_id = 0u64;
        let mut perturbed = false;
        for record in &records {
            max_op_id = max_op_id.max(record.op_id().unwrap_or(0));
            match record {
                JournalRecord::Header { .. } | JournalRecord::Applied { .. } => {}
                JournalRecord::ReclaimSession { .. } => hypervisor.reclaim_session(),
                JournalRecord::HvCreateVm { op_id, request } => {
                    check(
                        *op_id,
                        digest_response(&hypervisor.create_vm(request.clone())),
                    );
                }
                JournalRecord::HvRestoreSnapshot { op_id, request } => {
                    check(
                        *op_id,
                        digest_response(&hypervisor.restore_snapshot(*request)),
                    );
                }
                JournalRecord::HvFork { op_id, request } => {
                    check(*op_id, digest_response(&hypervisor.fork(request.clone())));
                }
                JournalRecord::HvInjectInputs { op_id, request } => {
                    check(
                        *op_id,
                        digest_response(&hypervisor.inject_inputs(request.clone())),
                    );
                }
                JournalRecord::HvRun { op_id, request } => {
                    check(*op_id, digest_response(&hypervisor.run(request.clone())));
                }
                JournalRecord::HvTakeSnapshot { op_id, request } => {
                    check(
                        *op_id,
                        digest_response(&hypervisor.take_snapshot(request.clone())),
                    );
                }
                JournalRecord::HvDestroyVm { op_id, request } => {
                    check(*op_id, digest_response(&hypervisor.destroy_vm(*request)));
                }
                JournalRecord::StCreateNode { op_id, request } => {
                    let mut request = request.clone();
                    if break_mode == Some(BreakMode::PerturbNode) && !perturbed {
                        perturbed = true;
                        request.progress_score =
                            orch_core::types::Score::new(request.progress_score.get() + 0.125)
                                .expect("perturbed score stays finite");
                    }
                    check(*op_id, digest_response(&store.create_node(request)));
                }
                JournalRecord::StUpdateNodes { op_id, request } => {
                    check(
                        *op_id,
                        digest_response(&store.update_nodes(request.clone())),
                    );
                }
                JournalRecord::StPutMetadata { op_id, request } => {
                    check(
                        *op_id,
                        digest_response(&store.put_metadata(request.clone())),
                    );
                }
                JournalRecord::StDeleteMetadata { op_id, request } => {
                    check(
                        *op_id,
                        digest_response(&store.delete_metadata(request.clone())),
                    );
                }
                JournalRecord::StPruneSubtree { op_id, request } => {
                    check(
                        *op_id,
                        digest_response(&store.prune_subtree(request.clone())),
                    );
                }
                JournalRecord::ScLoadFeatureMap { op_id, request } => {
                    check(
                        *op_id,
                        digest_response(&scorer.load_feature_map(request.clone())),
                    );
                }
                JournalRecord::ScLoadScoringProgram { op_id, request } => {
                    check(
                        *op_id,
                        digest_response(&scorer.load_scoring_program(request.clone())),
                    );
                }
                JournalRecord::ScScoreBatch { op_id, request } => {
                    if break_mode == Some(BreakMode::DropScorerReplay) {
                        continue;
                    }
                    check(
                        *op_id,
                        digest_response(&scorer.score_batch(request.clone())),
                    );
                }
                JournalRecord::ScCheckpointArchive { op_id, request } => {
                    check(
                        *op_id,
                        digest_response(&scorer.checkpoint_archive(request.clone())),
                    );
                }
                JournalRecord::ScRestoreArchive { op_id, request } => {
                    check(
                        *op_id,
                        digest_response(&scorer.restore_archive(request.clone())),
                    );
                }
                JournalRecord::ScReplayCommits { op_id, request } => {
                    if break_mode == Some(BreakMode::DropScorerReplay) {
                        continue;
                    }
                    check(
                        *op_id,
                        digest_response(&scorer.replay_commits(request.clone())),
                    );
                }
                JournalRecord::SyLoadMacroPack { op_id, request } => {
                    check(
                        *op_id,
                        digest_response(&synth.load_macro_pack(request.clone())),
                    );
                }
                JournalRecord::SyProposeBursts { op_id, request } => {
                    check(
                        *op_id,
                        digest_response(&synth.propose_bursts(request.clone())),
                    );
                }
                JournalRecord::SyMineMacros { op_id, request } => {
                    check(*op_id, digest_response(&synth.mine_macros(request.clone())));
                }
            }
        }

        // Read-only inspection stops here: no reclaim, no journal append, no
        // reopen — a fingerprint read must not mutate the state-dir (C3).
        // The returned world is journal-less and must not be driven further.
        if readonly {
            println!(
                "TIER2_SIM_RELOAD readonly frames={} truncated_bytes={}",
                stats.frames, stats.truncated_bytes
            );
            return Ok((
                Self {
                    hypervisor: Persistent::new(hypervisor, None),
                    scorer: Persistent::new(scorer, None),
                    store: Persistent::new(store, None),
                    synth: Persistent::new(synth, None),
                },
                stats,
            ));
        }

        // The worker observing the dead incarnation's connection drop —
        // journaled so the next incarnation replays it in order (D-T4).
        let mut journal = Journal::open_existing(dir, max_op_id + 1)?;
        hypervisor.reclaim_session();
        journal.append_op(
            |op_id| JournalRecord::ReclaimSession { op_id },
            RecordKind::Other,
        );

        // Surfaced for the harness: a forced torn write must show a nonzero
        // truncated_bytes here (W2.4).
        println!(
            "TIER2_SIM_RELOAD frames={} truncated_bytes={}",
            stats.frames, stats.truncated_bytes
        );

        let journal = Arc::new(Mutex::new(journal));
        Ok((
            Self {
                hypervisor: Persistent::new(hypervisor, Some(Arc::clone(&journal))),
                scorer: Persistent::new(scorer, Some(Arc::clone(&journal))),
                store: Persistent::new(store, Some(Arc::clone(&journal))),
                synth: Persistent::new(synth, Some(journal)),
            },
            stats,
        ))
    }

    /// Wraps the services in async adapters, the shape the runner consumes.
    #[must_use]
    pub fn into_world(self) -> PersistentWorld {
        PersistentWorld {
            hypervisor: SyncAdapter::new(self.hypervisor),
            scorer: SyncAdapter::new(self.scorer),
            store: SyncAdapter::new(self.store),
            synth: SyncAdapter::new(self.synth),
        }
    }
}

/// The persistent fake world in the shape `orchestratord` consumes: one
/// concrete type for both `--state-dir` modes (journal `None` when absent).
pub struct PersistentWorld {
    pub hypervisor: SyncAdapter<Persistent<FakeHypervisor>>,
    pub scorer: SyncAdapter<Persistent<FakeScorer>>,
    pub store: SyncAdapter<Persistent<InMemorySnapshotStore>>,
    pub synth: SyncAdapter<Persistent<FakeSynth>>,
}

impl PersistentWorld {
    #[must_use]
    pub fn observatory(&self) -> FakeObservatory {
        FakeObservatory::new()
    }
}
