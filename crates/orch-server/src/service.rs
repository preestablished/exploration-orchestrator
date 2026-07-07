//! The served `determinism.orchestrator.v1.ExplorationOrchestrator`
//! surface (API.md §1), wired over the same ports the runner uses.

use std::collections::HashMap;
use std::sync::Arc;

use orch_checkpoint::ExperimentState;
use orch_clients::{observatory::EventSink, snapshot_store::GetNodeRequest, ClientErrorKind};
use orch_core::types::NodeId;
use orch_proto::orchestrator_v1 as wire;
use orch_sched::ports::{AsyncHypervisor, AsyncScorer, AsyncStore, AsyncSynth};
use tokio::sync::Mutex;
use tonic::{Request, Response, Status};

use crate::{
    bringup::{ExperimentSources, SynthBringupPort},
    config::{config_hash, effective_config},
    experiment::{
        Control, ExperimentRunner, RunnerConfig, RunnerHandle, StartMode, StatusSnapshot,
    },
    SyncStoreAccess,
};

struct ExperimentEntry {
    handle: RunnerHandle,
    task: tokio::task::JoinHandle<()>,
    /// Stall-edge derivation for StreamProgress (window from the effective
    /// config at start).
    window_n: u32,
}

/// Resolves the artifact sources for one experiment (the fake worlds
/// template the synth config document by experiment id).
pub type SourcesFactory = Arc<dyn Fn(&str) -> ExperimentSources + Send + Sync>;

/// Service state: the four backend ports plus the experiment registry.
pub struct OrchestratorService<H, S, St, Sy, E>
where
    E: Clone,
{
    hypervisor: H,
    scorer: S,
    store: St,
    synth: Sy,
    sink: E,
    sources: SourcesFactory,
    producer_id: String,
    experiments: Mutex<HashMap<String, ExperimentEntry>>,
    /// Ids currently in bring-up (registry lock is not held across
    /// ExperimentRunner::start, which can take a while).
    starting: Mutex<std::collections::HashSet<String>>,
}

impl<H, S, St, Sy, E> OrchestratorService<H, S, St, Sy, E>
where
    H: AsyncHypervisor + Clone + 'static,
    S: AsyncScorer + Clone + 'static,
    St: AsyncStore + SyncStoreAccess + Clone + 'static,
    Sy: AsyncSynth + SynthBringupPort + Clone + 'static,
    E: EventSink + Clone + Send + Sync + 'static,
{
    pub fn new(
        hypervisor: H,
        scorer: S,
        store: St,
        synth: Sy,
        sink: E,
        sources: SourcesFactory,
        producer_id: impl Into<String>,
    ) -> Self {
        Self {
            hypervisor,
            scorer,
            store,
            synth,
            sink,
            sources,
            producer_id: producer_id.into(),
            experiments: Mutex::new(HashMap::new()),
            starting: Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Drains every running experiment: sends Stop and awaits a terminal
    /// state, so each runner writes its final checkpoint (SIGTERM path).
    pub async fn shutdown(&self) {
        let handles: Vec<(String, RunnerHandle)> = {
            let experiments = self.experiments.lock().await;
            experiments
                .iter()
                .filter(|(_, entry)| !entry.task.is_finished())
                .map(|(id, entry)| (id.clone(), entry.handle.clone()))
                .collect()
        };
        for (_, handle) in &handles {
            let _ = handle.send(Control::Stop);
        }
        for (_, handle) in handles {
            let mut watch = handle.watch();
            loop {
                let state = watch.borrow_and_update().state;
                if !matches!(
                    state,
                    ExperimentState::Running | ExperimentState::Pending | ExperimentState::Paused
                ) {
                    break;
                }
                if watch.changed().await.is_err() {
                    break;
                }
            }
        }
    }

    async fn entry_status(&self, experiment_id: &str) -> Result<StatusSnapshot, Status> {
        let experiments = self.experiments.lock().await;
        let entry = experiments
            .get(experiment_id)
            .ok_or_else(|| Status::not_found(format!("unknown experiment {experiment_id}")))?;
        Ok(entry.handle.status())
    }
}

fn state_to_wire(state: ExperimentState) -> wire::ExperimentState {
    match state {
        ExperimentState::Pending => wire::ExperimentState::Pending,
        ExperimentState::Running => wire::ExperimentState::Running,
        ExperimentState::Paused => wire::ExperimentState::Paused,
        ExperimentState::Stopped => wire::ExperimentState::Stopped,
        ExperimentState::GoalReached => wire::ExperimentState::GoalReached,
        ExperimentState::BudgetExhausted => wire::ExperimentState::BudgetExhausted,
        ExperimentState::Failed => wire::ExperimentState::Failed,
    }
}

fn stats_to_wire(snapshot: &StatusSnapshot) -> wire::ExperimentStats {
    wire::ExperimentStats {
        expansions: snapshot.expansions,
        nodes_committed: snapshot.nodes,
        children_discarded_dup: snapshot.children_discarded_dup,
        children_discarded_regression: snapshot.children_discarded_regression,
        frontier_size: snapshot.frontier_size,
        archive_cells: 0, // full M5 surface later
        best_score: snapshot.best_score.unwrap_or_default(),
        best_node_id: snapshot.best_node.map_or(0, NodeId::get),
        best_stage: 0, // full M5 surface later
        guest_instructions_used: snapshot.guest_instructions_used,
        wall_clock_seconds: 0, // real binary reports wall time
        batch_seq: snapshot.batch_seq,
        slots_utilization: 0.0, // full M5 surface later
    }
}

fn status_to_wire(experiment_id: &str, snapshot: &StatusSnapshot) -> wire::ExperimentStatus {
    wire::ExperimentStatus {
        experiment_id: experiment_id.to_owned(),
        state: state_to_wire(snapshot.state) as i32,
        stats: Some(stats_to_wire(snapshot)),
        plateau: Some(wire::PlateauStatus {
            level: snapshot.escalation_level,
            expansions_since_improvement: snapshot.expansions_since_improvement,
        }),
        goal_node_ids: snapshot.goal_nodes.iter().map(|node| node.get()).collect(),
        failure_reason: snapshot.failure_reason.clone().unwrap_or_default(),
    }
}

#[tonic::async_trait]
impl<H, S, St, Sy, E> wire::exploration_orchestrator_server::ExplorationOrchestrator
    for OrchestratorService<H, S, St, Sy, E>
where
    H: AsyncHypervisor + Clone + 'static,
    S: AsyncScorer + Clone + 'static,
    St: AsyncStore + SyncStoreAccess + Clone + 'static,
    Sy: AsyncSynth + SynthBringupPort + Clone + 'static,
    E: EventSink + Clone + Send + Sync + 'static,
{
    async fn start_experiment(
        &self,
        request: Request<wire::StartExperimentRequest>,
    ) -> Result<Response<wire::StartExperimentResponse>, Status> {
        let request = request.into_inner();
        if request.experiment_id.is_empty() {
            return Err(Status::invalid_argument("experiment_id is required"));
        }
        let Some(config) = request.config.as_ref() else {
            return Err(Status::invalid_argument("config is required"));
        };

        // Validation lists EVERY bad field (API.md §1).
        let effective = effective_config(config);
        let violations = effective.validate_all();
        if !violations.is_empty() {
            let details: Vec<String> = violations
                .iter()
                .map(|violation| violation.to_string())
                .collect();
            return Err(Status::invalid_argument(format!(
                "config validation failed: {}",
                details.join("; ")
            )));
        }

        {
            let experiments = self.experiments.lock().await;
            if let Some(entry) = experiments.get(&request.experiment_id) {
                let running = !entry.task.is_finished();
                if running && !request.resume_if_exists {
                    return Err(Status::already_exists(format!(
                        "experiment {} is already running",
                        request.experiment_id
                    )));
                }
                let snapshot = entry.handle.status();
                if running {
                    return Ok(Response::new(wire::StartExperimentResponse {
                        experiment_id: request.experiment_id,
                        state: state_to_wire(snapshot.state) as i32,
                        resumed_at_batch_seq: snapshot.checkpointed_batch_seq,
                    }));
                }
                // Finished id: experiment_id is the idempotency key, so a
                // resubmit returns the terminal status instead of silently
                // re-running and overwriting the record (review finding).
                // resume_if_exists explicitly opts back in.
                if !request.resume_if_exists {
                    return Ok(Response::new(wire::StartExperimentResponse {
                        experiment_id: request.experiment_id,
                        state: state_to_wire(snapshot.state) as i32,
                        resumed_at_batch_seq: snapshot.checkpointed_batch_seq,
                    }));
                }
            }
            // Reserve the id so bring-up can run without holding the
            // registry lock (bring-up is slow; the lock serialises RPCs).
            let mut starting = self.starting.lock().await;
            if !starting.insert(request.experiment_id.clone()) {
                return Err(Status::already_exists(format!(
                    "experiment {} is already starting",
                    request.experiment_id
                )));
            }
        }

        let run_id = if request.run_id.is_empty() {
            request.experiment_id.clone()
        } else {
            request.run_id.clone()
        };
        let window_n = effective.plateau.window_n;
        let runner_config = RunnerConfig {
            experiment_id: request.experiment_id.clone(),
            run_id,
            producer_id: self.producer_id.clone(),
            config_hash: config_hash(&effective),
            config: effective,
        };

        let started = ExperimentRunner::start(
            runner_config,
            (self.sources)(&request.experiment_id),
            self.hypervisor.clone(),
            self.scorer.clone(),
            self.store.clone(),
            self.synth.clone(),
            self.sink.clone(),
            None,
        )
        .await;
        let (runner, handle, mode) = match started {
            Ok(parts) => parts,
            Err(error) => {
                self.starting.lock().await.remove(&request.experiment_id);
                return Err(match error.kind() {
                    ClientErrorKind::FailedPrecondition => {
                        Status::failed_precondition(error.message().to_owned())
                    }
                    ClientErrorKind::InvalidRequest => {
                        Status::invalid_argument(error.message().to_owned())
                    }
                    _ => Status::internal(error.message().to_owned()),
                });
            }
        };

        let resumed_at = match mode {
            StartMode::Fresh => 0,
            StartMode::Resumed {
                checkpoint_batch_seq,
            } => checkpoint_batch_seq,
        };
        let task = tokio::spawn(async move {
            let _ = runner.run().await;
        });
        {
            let mut experiments = self.experiments.lock().await;
            experiments.insert(
                request.experiment_id.clone(),
                ExperimentEntry {
                    handle,
                    task,
                    window_n,
                },
            );
            self.starting.lock().await.remove(&request.experiment_id);
        }

        Ok(Response::new(wire::StartExperimentResponse {
            experiment_id: request.experiment_id,
            state: wire::ExperimentState::Running as i32,
            resumed_at_batch_seq: resumed_at,
        }))
    }

    async fn pause_experiment(
        &self,
        request: Request<wire::PauseExperimentRequest>,
    ) -> Result<Response<wire::PauseExperimentResponse>, Status> {
        let request = request.into_inner();
        let (handle, mut watch) = {
            let experiments = self.experiments.lock().await;
            let entry = experiments.get(&request.experiment_id).ok_or_else(|| {
                Status::not_found(format!("unknown experiment {}", request.experiment_id))
            })?;
            (entry.handle.clone(), entry.handle.watch())
        };
        handle
            .send(Control::Pause)
            .map_err(|error| Status::failed_precondition(error.message().to_owned()))?;
        loop {
            let snapshot = watch.borrow_and_update().clone();
            match snapshot.state {
                ExperimentState::Paused => {
                    return Ok(Response::new(wire::PauseExperimentResponse {
                        state: wire::ExperimentState::Paused as i32,
                        checkpointed_batch_seq: snapshot.checkpointed_batch_seq,
                    }));
                }
                ExperimentState::Running | ExperimentState::Pending => {}
                terminal => {
                    return Ok(Response::new(wire::PauseExperimentResponse {
                        state: state_to_wire(terminal) as i32,
                        checkpointed_batch_seq: snapshot.checkpointed_batch_seq,
                    }));
                }
            }
            if watch.changed().await.is_err() {
                return Err(Status::internal("runner stopped while pausing"));
            }
        }
    }

    async fn resume_experiment(
        &self,
        request: Request<wire::ResumeExperimentRequest>,
    ) -> Result<Response<wire::ResumeExperimentResponse>, Status> {
        let request = request.into_inner();
        let handle = {
            let experiments = self.experiments.lock().await;
            experiments
                .get(&request.experiment_id)
                .ok_or_else(|| {
                    Status::not_found(format!("unknown experiment {}", request.experiment_id))
                })?
                .handle
                .clone()
        };
        handle
            .send(Control::Resume)
            .map_err(|error| Status::failed_precondition(error.message().to_owned()))?;
        Ok(Response::new(wire::ResumeExperimentResponse {
            state: wire::ExperimentState::Running as i32,
        }))
    }

    async fn stop_experiment(
        &self,
        request: Request<wire::StopExperimentRequest>,
    ) -> Result<Response<wire::StopExperimentResponse>, Status> {
        let request = request.into_inner();
        let handle = {
            let experiments = self.experiments.lock().await;
            let entry = experiments.get(&request.experiment_id).ok_or_else(|| {
                Status::not_found(format!("unknown experiment {}", request.experiment_id))
            })?;
            entry.handle.clone()
        };
        // abandon_inflight: on fakes the drain is bounded, so both stop
        // flavors drain (disclosed; the real cancel path is M6 work).
        let _ = handle.send(Control::Stop);
        // Wait for the runner to finish its final checkpoint.
        let mut watch = handle.watch();
        loop {
            let snapshot = watch.borrow_and_update().clone();
            if !matches!(
                snapshot.state,
                ExperimentState::Running | ExperimentState::Pending | ExperimentState::Paused
            ) {
                let stats = stats_to_wire(&snapshot);
                return Ok(Response::new(wire::StopExperimentResponse {
                    state: state_to_wire(snapshot.state) as i32,
                    final_stats: Some(stats),
                }));
            }
            if watch.changed().await.is_err() {
                let snapshot = watch.borrow().clone();
                return Ok(Response::new(wire::StopExperimentResponse {
                    state: state_to_wire(snapshot.state) as i32,
                    final_stats: Some(stats_to_wire(&snapshot)),
                }));
            }
        }
    }

    async fn get_experiment_status(
        &self,
        request: Request<wire::GetExperimentStatusRequest>,
    ) -> Result<Response<wire::ExperimentStatus>, Status> {
        let request = request.into_inner();
        let snapshot = self.entry_status(&request.experiment_id).await?;
        Ok(Response::new(status_to_wire(
            &request.experiment_id,
            &snapshot,
        )))
    }

    type StreamProgressStream = std::pin::Pin<
        Box<dyn futures_core::Stream<Item = Result<wire::ProgressEvent, Status>> + Send>,
    >;

    async fn stream_progress(
        &self,
        request: Request<wire::StreamProgressRequest>,
    ) -> Result<Response<Self::StreamProgressStream>, Status> {
        let request = request.into_inner();
        let (handle, store, window_n) = {
            let experiments = self.experiments.lock().await;
            let entry = experiments.get(&request.experiment_id).ok_or_else(|| {
                Status::not_found(format!("unknown experiment {}", request.experiment_id))
            })?;
            (entry.handle.clone(), self.store.clone(), entry.window_n)
        };
        let experiment_id = request.experiment_id.clone();
        let min_interval =
            std::time::Duration::from_millis(u64::from(if request.min_interval_ms == 0 {
                1000
            } else {
                request.min_interval_ms
            }));

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<wire::ProgressEvent, Status>>(16);
        tokio::spawn(async move {
            let mut watch = handle.watch();
            let mut previous = watch.borrow_and_update().clone();
            // Every goal gets its own edge (on_goal = CONTINUE keeps
            // committing goals after the first); the latch is a count, not
            // a bool (review finding, both reviewers).
            let mut goals_emitted = previous.goal_nodes.len();
            // Initial heartbeat (latest goal re-emitted on reconnect,
            // API.md §1).
            let mut initial = wire::ProgressEvent {
                at: Some(now_timestamp()),
                status: Some(status_to_wire(&experiment_id, &previous)),
                edge: None,
            };
            if let Some(goal) = previous.goal_nodes.last().copied() {
                initial.edge = goal_edge(&experiment_id, goal, &previous, &store).await;
            }
            if tx.send(Ok(initial)).await.is_err() {
                return;
            }
            let mut last_sent = tokio::time::Instant::now();
            loop {
                let changed = tokio::time::timeout(min_interval, watch.changed()).await;
                let snapshot = watch.borrow_and_update().clone();
                let is_transition = snapshot.state != previous.state
                    || snapshot.escalation_level != previous.escalation_level
                    || snapshot.goal_nodes.len() != previous.goal_nodes.len();
                let heartbeat_due = last_sent.elapsed() >= min_interval;
                if is_transition || heartbeat_due {
                    let mut event = wire::ProgressEvent {
                        at: Some(now_timestamp()),
                        status: Some(status_to_wire(&experiment_id, &snapshot)),
                        edge: None,
                    };
                    if snapshot.goal_nodes.len() > goals_emitted {
                        let goal = snapshot.goal_nodes[goals_emitted];
                        event.edge = goal_edge(&experiment_id, goal, &snapshot, &store).await;
                        goals_emitted += 1;
                    } else if snapshot.state == ExperimentState::BudgetExhausted
                        && previous.state != ExperimentState::BudgetExhausted
                    {
                        event.edge =
                            Some(wire::progress_event::Edge::Budget(wire::BudgetExhausted {
                                budget: snapshot
                                    .failure_reason
                                    .clone()
                                    .unwrap_or_default()
                                    .replace("budget-exhausted: ", ""),
                            }));
                    } else if snapshot.escalation_level > previous.escalation_level {
                        event.edge = Some(wire::progress_event::Edge::Escalation(
                            wire::EscalationChanged {
                                from_level: previous.escalation_level,
                                to_level: snapshot.escalation_level,
                            },
                        ));
                    } else if window_n > 0
                        && snapshot.expansions_since_improvement / u64::from(window_n)
                            > previous.expansions_since_improvement / u64::from(window_n)
                    {
                        // A stall window completed without an escalation
                        // change (review finding: the StallDetected edge was
                        // never emitted).
                        event.edge = Some(wire::progress_event::Edge::Stall(wire::StallDetected {
                            level: snapshot.escalation_level,
                            best_score: snapshot.best_score.unwrap_or_default(),
                            window_n: u64::from(window_n),
                        }));
                    }
                    if tx.send(Ok(event)).await.is_err() {
                        return;
                    }
                    last_sent = tokio::time::Instant::now();
                    previous = snapshot.clone();
                }
                if changed.is_ok_and(|result| result.is_err()) {
                    // Runner ended; final status already sent above.
                    return;
                }
            }
        });

        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }
}

async fn goal_edge<St>(
    experiment_id: &str,
    goal: NodeId,
    _snapshot: &StatusSnapshot,
    store: &St,
) -> Option<wire::progress_event::Edge>
where
    St: AsyncStore,
{
    let meta = store
        .get_node(GetNodeRequest {
            experiment_id: experiment_id.to_owned(),
            node_id: goal,
        })
        .await
        .ok()?;
    Some(wire::progress_event::Edge::Goal(wire::GoalReached {
        node_id: goal.get(),
        snapshot_ref: meta.node.snapshot_ref.as_bytes().to_vec(),
        score: meta.node.progress_score.get(),
        depth: u32::try_from(meta.node.depth).unwrap_or(u32::MAX),
    }))
}

fn now_timestamp() -> prost_types::Timestamp {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    prost_types::Timestamp {
        seconds: i64::try_from(now.as_secs()).unwrap_or(i64::MAX),
        nanos: i32::try_from(now.subsec_nanos()).unwrap_or(0),
    }
}
