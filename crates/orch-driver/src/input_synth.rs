//! Driver-owned input-synth adapter, bring-up, request, and fingerprint helpers.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
    time::Duration,
};

use orch_clients::{
    input_synth::{
        Burst, BurstBody, BurstId, ConfigFingerprint, DegradedGenerator, DocumentKind, EventBurst,
        FieldValue, GeneratorKind, GrammarEvent, GrammarField, HealthRequest, HealthResponse,
        HealthStatus, InputSynthClient, LoadMacroPackRequest, LoadMacroPackResponse,
        LoadMacroPackSource, MacroProvenance, MineMacrosRequest, MineMacrosResponse,
        MinedMacroStats, MiningParams, ModelKind, MutationOp, MutationProvenance, NodeContext,
        PadBurst, PadSegment, PathSample, PolicyProvenance, ProposeBurstsRequest,
        ProposeBurstsResponse, Provenance, ProvenancedBurst, ScoredBurst, BURST_ID_LEN,
        CONFIG_FINGERPRINT_LEN,
    },
    snapshot_store::SnapshotStoreClient,
    ClientError, ClientErrorKind, ClientResult,
};
use orch_core::{
    rng::derive_synth_request_seed,
    types::{FiniteF64, FrameCount, NodeId, Novelty, Score, SnapshotRef, DIGEST_LEN},
};
use orch_proto::inputsynth::v1 as wire;
use tokio::runtime::Runtime;
use tonic::{
    transport::{Channel, Endpoint},
    Code, Request, Status,
};

use crate::node_attrs::{build_input_synth_node_context, NodeContextLimits};

type WireInputSynthClient = wire::input_synthesizer_client::InputSynthesizerClient<Channel>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedInputSynthConfig {
    pub endpoint: String,
    pub deadline: Duration,
    pub retry_budget: u32,
}

impl GeneratedInputSynthConfig {
    #[must_use]
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            deadline: Duration::from_secs(10),
            retry_budget: 0,
        }
    }

    #[must_use]
    pub const fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    #[must_use]
    pub const fn with_retry_budget(mut self, retry_budget: u32) -> Self {
        self.retry_budget = retry_budget;
        self
    }
}

pub struct GeneratedInputSynthClient {
    inner: Mutex<WireInputSynthClient>,
    runtime: Runtime,
    config: GeneratedInputSynthConfig,
}

impl GeneratedInputSynthClient {
    pub fn connect(config: GeneratedInputSynthConfig) -> ClientResult<Self> {
        let endpoint = Endpoint::from_shared(config.endpoint.clone()).map_err(|error| {
            ClientError::new(
                ClientErrorKind::InvalidRequest,
                format!("invalid input-synth endpoint: {error}"),
            )
        })?;
        let endpoint = endpoint.timeout(config.deadline);
        let runtime = runtime()?;
        let channel = runtime
            .block_on(endpoint.connect())
            .map_err(transport_error)?;

        Ok(Self::from_channel(channel, runtime, config))
    }

    #[must_use]
    pub fn from_channel(
        channel: Channel,
        runtime: Runtime,
        config: GeneratedInputSynthConfig,
    ) -> Self {
        Self {
            inner: Mutex::new(WireInputSynthClient::new(channel)),
            runtime,
            config,
        }
    }

    #[must_use]
    pub const fn config(&self) -> &GeneratedInputSynthConfig {
        &self.config
    }
}

impl InputSynthClient for GeneratedInputSynthClient {
    fn load_macro_pack(
        &mut self,
        request: LoadMacroPackRequest,
    ) -> ClientResult<LoadMacroPackResponse> {
        let wire_request = dto_to_wire_load_macro_pack_request(&request)?;
        let mut attempts = 0;
        let response = loop {
            let mut client = self.lock_client()?;
            let result = self
                .runtime
                .block_on(client.load_macro_pack(request_with_deadline(
                    wire_request.clone(),
                    self.config.deadline,
                )));
            match result {
                Ok(response) => break response,
                Err(status)
                    if is_transport_retryable(&status) && attempts < self.config.retry_budget =>
                {
                    attempts += 1;
                }
                Err(status) => return Err(status_to_client_error(status)),
            }
        };

        wire_to_dto_load_macro_pack_response(response.into_inner())
    }

    fn health(&self, request: HealthRequest) -> ClientResult<HealthResponse> {
        let wire_request = dto_to_wire_health_request(request)?;
        let mut attempts = 0;
        let response = loop {
            let mut client = self.lock_client()?;
            let result = self.runtime.block_on(client.health(request_with_deadline(
                wire_request.clone(),
                self.config.deadline,
            )));
            match result {
                Ok(response) => break response,
                Err(status)
                    if is_transport_retryable(&status) && attempts < self.config.retry_budget =>
                {
                    attempts += 1;
                }
                Err(status) => return Err(status_to_client_error(status)),
            }
        };

        wire_to_dto_health_response(response.into_inner())
    }

    fn propose_bursts(
        &mut self,
        request: ProposeBurstsRequest,
    ) -> ClientResult<ProposeBurstsResponse> {
        let wire_request = dto_to_wire_propose_bursts_request(&request)?;
        let mut attempts = 0;
        let response = loop {
            let mut client = self.lock_client()?;
            let result = self
                .runtime
                .block_on(client.propose_bursts(request_with_deadline(
                    wire_request.clone(),
                    self.config.deadline,
                )));
            match result {
                Ok(response) => break response,
                Err(status)
                    if is_transport_retryable(&status) && attempts < self.config.retry_budget =>
                {
                    attempts += 1;
                }
                Err(status) => return Err(status_to_client_error(status)),
            }
        };

        wire_to_dto_propose_bursts_response(&request, response.into_inner())
    }

    fn mine_macros(&mut self, request: MineMacrosRequest) -> ClientResult<MineMacrosResponse> {
        let wire_request = dto_to_wire_mine_macros_request(&request)?;
        let mut attempts = 0;
        let response = loop {
            let mut client = self.lock_client()?;
            let result = self
                .runtime
                .block_on(client.mine_macros(request_with_deadline(
                    wire_request.clone(),
                    self.config.deadline,
                )));
            match result {
                Ok(response) => break response,
                Err(status)
                    if is_transport_retryable(&status) && attempts < self.config.retry_budget =>
                {
                    attempts += 1;
                }
                Err(status) => return Err(status_to_client_error(status)),
            }
        };

        wire_to_dto_mine_macros_response(response.into_inner())
    }
}

impl GeneratedInputSynthClient {
    fn lock_client(&self) -> ClientResult<std::sync::MutexGuard<'_, WireInputSynthClient>> {
        self.inner.lock().map_err(|_| {
            ClientError::new(
                ClientErrorKind::Internal,
                "input-synth client mutex poisoned",
            )
        })
    }
}

pub fn dto_to_wire_load_macro_pack_request(
    request: &LoadMacroPackRequest,
) -> ClientResult<wire::LoadMacroPackRequest> {
    let source = match &request.source {
        LoadMacroPackSource::DocumentYaml(bytes) => {
            wire::load_macro_pack_request::Source::DocumentYaml(bytes.clone())
        }
        LoadMacroPackSource::ArtifactRef(reference) => {
            if reference.trim().is_empty() {
                return Err(ClientError::new(
                    ClientErrorKind::InvalidRequest,
                    "artifact_ref must not be empty",
                ));
            }
            wire::load_macro_pack_request::Source::ArtifactRef(reference.clone())
        }
    };

    Ok(wire::LoadMacroPackRequest {
        source: Some(source),
        kind: dto_document_kind_to_wire(request.kind) as i32,
    })
}

pub fn dto_to_wire_health_request(_request: HealthRequest) -> ClientResult<wire::HealthRequest> {
    Ok(wire::HealthRequest {})
}

pub fn dto_to_wire_propose_bursts_request(
    request: &ProposeBurstsRequest,
) -> ClientResult<wire::ProposeBurstsRequest> {
    validate_propose_bursts_request(request)?;
    Ok(wire::ProposeBurstsRequest {
        experiment_id: request.experiment_id.clone(),
        node_context: Some(dto_to_wire_node_context(&request.node_context)?),
        k: request.k,
        length_hint: request.length_hint.get(),
        seed: request.seed,
        model: dto_model_kind_to_wire(request.model) as i32,
        config_overrides_yaml: request.config_overrides_yaml.clone(),
    })
}

pub fn dto_to_wire_mine_macros_request(
    request: &MineMacrosRequest,
) -> ClientResult<wire::MineMacrosRequest> {
    if request.experiment_id.trim().is_empty() {
        return Err(ClientError::new(
            ClientErrorKind::InvalidRequest,
            "experiment_id must not be empty",
        ));
    }

    Ok(wire::MineMacrosRequest {
        experiment_id: request.experiment_id.clone(),
        paths: request
            .paths
            .iter()
            .map(dto_to_wire_path_sample)
            .collect::<ClientResult<_>>()?,
        params: Some(dto_to_wire_mining_params(&request.params)?),
    })
}

pub fn wire_to_dto_load_macro_pack_response(
    response: wire::LoadMacroPackResponse,
) -> ClientResult<LoadMacroPackResponse> {
    Ok(LoadMacroPackResponse {
        document_id: response.document_id,
        items_loaded: response.items_loaded,
        warnings: response.warnings,
    })
}

pub fn wire_to_dto_health_response(response: wire::HealthResponse) -> ClientResult<HealthResponse> {
    Ok(HealthResponse {
        status: wire_health_status_to_dto(response.status)?,
        synth_version: response.synth_version,
        loaded_packs: response.loaded_packs,
        loaded_experiments: response.loaded_experiments,
        policy_endpoint_up: response.policy_endpoint_up,
        policy_deterministic: response.policy_deterministic,
        mining_in_progress: response.mining_in_progress,
    })
}

pub fn wire_to_dto_propose_bursts_response(
    request: &ProposeBurstsRequest,
    response: wire::ProposeBurstsResponse,
) -> ClientResult<ProposeBurstsResponse> {
    let config_fingerprint = config_fingerprint_from_wire(&response.config_fingerprint)?;
    let dto = ProposeBurstsResponse {
        bursts: response
            .bursts
            .into_iter()
            .map(wire_to_dto_provenanced_burst)
            .collect::<ClientResult<_>>()?,
        config_fingerprint,
        synth_version: response.synth_version,
        seed: response.seed,
        degraded: response
            .degraded
            .into_iter()
            .map(wire_to_dto_degraded_generator)
            .collect::<ClientResult<_>>()?,
    };
    validate_propose_bursts_response(request, &dto)?;
    Ok(dto)
}

pub fn wire_to_dto_mine_macros_response(
    response: wire::MineMacrosResponse,
) -> ClientResult<MineMacrosResponse> {
    Ok(MineMacrosResponse {
        macro_pack_yaml: response.macro_pack_yaml,
        pack_id: response.pack_id,
        stats: response
            .stats
            .into_iter()
            .map(wire_to_dto_mined_macro_stats)
            .collect::<ClientResult<_>>()?,
        paths_used: response.paths_used,
        tokens_scanned: response.tokens_scanned,
    })
}

pub fn status_to_client_error(status: Status) -> ClientError {
    ClientError::new(
        tonic_code_to_client_error_kind(status.code()),
        status.message().to_owned(),
    )
}

fn is_transport_retryable(status: &Status) -> bool {
    matches!(status.code(), Code::Unavailable | Code::DeadlineExceeded)
}

#[must_use]
pub const fn tonic_code_to_client_error_kind(code: Code) -> ClientErrorKind {
    match code {
        Code::InvalidArgument => ClientErrorKind::InvalidRequest,
        Code::FailedPrecondition => ClientErrorKind::FailedPrecondition,
        Code::NotFound => ClientErrorKind::NotFound,
        Code::AlreadyExists => ClientErrorKind::AlreadyExists,
        Code::ResourceExhausted => ClientErrorKind::ResourceExhausted,
        Code::Unavailable | Code::DeadlineExceeded => ClientErrorKind::Unavailable,
        Code::Internal | Code::DataLoss | Code::Unknown => ClientErrorKind::Internal,
        _ => ClientErrorKind::Internal,
    }
}

pub fn node_id_to_wire(node_id: NodeId) -> String {
    node_id.get().to_string()
}

pub fn node_id_from_wire(value: &str, kind: ClientErrorKind) -> ClientResult<NodeId> {
    if value.is_empty() {
        return Err(ClientError::new(kind, "node_id must not be empty"));
    }
    if value.starts_with('-') || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ClientError::new(
            kind,
            format!("node_id '{value}' is not an unsigned decimal integer"),
        ));
    }
    let parsed = value
        .parse::<u64>()
        .map_err(|_| ClientError::new(kind, format!("node_id '{value}' is outside u64 range")))?;
    Ok(NodeId::new(parsed))
}

pub fn snapshot_ref_to_wire(snapshot_ref: SnapshotRef) -> String {
    hex_lower(snapshot_ref.as_bytes())
}

pub fn snapshot_ref_from_wire(value: &str, kind: ClientErrorKind) -> ClientResult<SnapshotRef> {
    let bytes = decode_fixed_hex::<DIGEST_LEN>(value, kind, "snapshot_ref")?;
    Ok(SnapshotRef::new(bytes))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SynthBringup {
    pub experiment_id: String,
    pub experiment_config_source: LoadMacroPackSource,
    pub macro_pack_sources: Vec<LoadMacroPackSource>,
    pub required_pack_ids: BTreeSet<String>,
}

impl SynthBringup {
    #[must_use]
    pub fn new(
        experiment_id: impl Into<String>,
        experiment_config_source: LoadMacroPackSource,
        macro_pack_sources: Vec<LoadMacroPackSource>,
        required_pack_ids: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            experiment_id: experiment_id.into(),
            experiment_config_source,
            macro_pack_sources,
            required_pack_ids: required_pack_ids.into_iter().collect(),
        }
    }

    pub fn from_sources(
        experiment_id: impl Into<String>,
        experiment_config_source: LoadMacroPackSource,
        macro_pack_sources: Vec<LoadMacroPackSource>,
    ) -> ClientResult<Self> {
        let mut required_pack_ids = BTreeSet::new();
        if let LoadMacroPackSource::DocumentYaml(bytes) = &experiment_config_source {
            required_pack_ids.extend(parse_required_macro_pack_ids(bytes)?);
        }

        Ok(Self::new(
            experiment_id,
            experiment_config_source,
            macro_pack_sources,
            required_pack_ids,
        ))
    }

    pub fn run<C: InputSynthClient>(&self, client: &mut C) -> ClientResult<SynthBringupReport> {
        if self.experiment_id.trim().is_empty() {
            return Err(ClientError::new(
                ClientErrorKind::InvalidRequest,
                "experiment_id must not be empty",
            ));
        }

        let experiment_config = client.load_macro_pack(LoadMacroPackRequest {
            source: self.experiment_config_source.clone(),
            kind: DocumentKind::ExperimentConfig,
        })?;

        let mut macro_pack_documents = Vec::with_capacity(self.macro_pack_sources.len());
        let mut expected_loaded_packs = self.required_pack_ids.clone();
        for source in &self.macro_pack_sources {
            let response = client.load_macro_pack(LoadMacroPackRequest {
                source: source.clone(),
                kind: DocumentKind::MacroPack,
            })?;
            expected_loaded_packs.insert(response.document_id.clone());
            macro_pack_documents.push(response);
        }

        let health = client.health(HealthRequest)?;
        if health.status == HealthStatus::NotServing {
            return Err(ClientError::new(
                ClientErrorKind::FailedPrecondition,
                "input synthesizer is not serving after bring-up",
            ));
        }

        let loaded_packs = health.loaded_packs.iter().cloned().collect::<BTreeSet<_>>();
        let missing = expected_loaded_packs
            .difference(&loaded_packs)
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(ClientError::new(
                ClientErrorKind::FailedPrecondition,
                format!(
                    "input synthesizer health is missing loaded macro packs: {}",
                    missing.join(", ")
                ),
            ));
        }

        Ok(SynthBringupReport {
            experiment_config,
            macro_pack_documents,
            required_pack_ids: self.required_pack_ids.clone(),
            health,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SynthBringupReport {
    pub experiment_config: LoadMacroPackResponse,
    pub macro_pack_documents: Vec<LoadMacroPackResponse>,
    pub required_pack_ids: BTreeSet<String>,
    pub health: HealthResponse,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SynthProfile {
    pub experiment_id: String,
    pub model: SynthProfileModel,
    pub config_overrides_yaml: Vec<u8>,
}

impl SynthProfile {
    #[must_use]
    pub fn from_request(request: &ProposeBurstsRequest) -> Self {
        Self {
            experiment_id: request.experiment_id.clone(),
            model: SynthProfileModel::from(request.model),
            config_overrides_yaml: request.config_overrides_yaml.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SynthProfileModel {
    Pad,
    EventGrammar,
}

impl From<ModelKind> for SynthProfileModel {
    fn from(value: ModelKind) -> Self {
        match value {
            ModelKind::Pad => Self::Pad,
            ModelKind::EventGrammar => Self::EventGrammar,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FingerprintRegistry {
    expected: BTreeMap<SynthProfile, ConfigFingerprint>,
}

impl FingerprintRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn check_or_insert(
        &mut self,
        profile: SynthProfile,
        actual: ConfigFingerprint,
    ) -> Result<FingerprintCheck, FingerprintMismatch> {
        match self.expected.get(&profile).copied() {
            None => {
                self.expected.insert(profile, actual);
                Ok(FingerprintCheck::Inserted)
            }
            Some(expected) if expected == actual => Ok(FingerprintCheck::Matched),
            Some(expected) => Err(FingerprintMismatch {
                profile,
                expected,
                actual,
            }),
        }
    }

    #[must_use]
    pub fn expected_fingerprint(&self, profile: &SynthProfile) -> Option<ConfigFingerprint> {
        self.expected.get(profile).copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FingerprintCheck {
    Inserted,
    Matched,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FingerprintMismatch {
    pub profile: SynthProfile,
    pub expected: ConfigFingerprint,
    pub actual: ConfigFingerprint,
}

impl FingerprintMismatch {
    #[must_use]
    pub fn into_client_error(self) -> ClientError {
        ClientError::new(
            ClientErrorKind::FailedPrecondition,
            format!(
                "input synthesizer config fingerprint mismatch for experiment '{}' model {:?}: expected {}, got {}",
                self.profile.experiment_id,
                self.profile.model,
                fingerprint_hex(self.expected),
                fingerprint_hex(self.actual)
            ),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposeBurstsBuildSpec {
    pub experiment_id: String,
    pub node_id: NodeId,
    pub k: u32,
    pub length_hint: FrameCount,
    pub experiment_seed: u64,
    pub batch_seq: u64,
    pub model: ModelKind,
    pub config_overrides_yaml: Vec<u8>,
    pub context_limits: NodeContextLimits,
}

pub fn build_propose_bursts_request<S: SnapshotStoreClient>(
    store: &S,
    spec: ProposeBurstsBuildSpec,
) -> ClientResult<ProposeBurstsRequest> {
    let node_context = build_input_synth_node_context(
        store,
        &spec.experiment_id,
        spec.node_id,
        spec.context_limits,
    )?;
    let request = ProposeBurstsRequest {
        experiment_id: spec.experiment_id,
        node_context,
        k: spec.k,
        length_hint: spec.length_hint,
        seed: derive_synth_request_seed(spec.experiment_seed, spec.batch_seq),
        model: spec.model,
        config_overrides_yaml: spec.config_overrides_yaml,
    };
    validate_propose_bursts_request(&request)?;
    Ok(request)
}

pub fn validate_propose_bursts_request(request: &ProposeBurstsRequest) -> ClientResult<()> {
    if request.experiment_id.trim().is_empty() {
        return Err(ClientError::new(
            ClientErrorKind::InvalidRequest,
            "experiment_id must not be empty",
        ));
    }
    if request.k == 0 {
        return Err(ClientError::new(
            ClientErrorKind::InvalidRequest,
            "k must be nonzero",
        ));
    }
    if !request.node_context.frame_embedding.is_empty() {
        return Err(ClientError::new(
            ClientErrorKind::InvalidRequest,
            "node_context.frame_embedding must stay empty until Phase 8 defines f64-to-f32 policy",
        ));
    }
    Ok(())
}

pub fn validate_propose_bursts_response(
    request: &ProposeBurstsRequest,
    response: &ProposeBurstsResponse,
) -> ClientResult<()> {
    if response.seed != request.seed {
        return Err(ClientError::new(
            ClientErrorKind::DataLoss,
            format!(
                "input synthesizer response seed {} did not echo request seed {}",
                response.seed, request.seed
            ),
        ));
    }
    if response.bursts.len() != request.k as usize {
        return Err(ClientError::new(
            ClientErrorKind::DataLoss,
            format!(
                "input synthesizer returned {} bursts for k={}",
                response.bursts.len(),
                request.k
            ),
        ));
    }

    for (index, burst) in response.bursts.iter().enumerate() {
        if burst.provenance.config_fingerprint != response.config_fingerprint {
            return Err(ClientError::new(
                ClientErrorKind::DataLoss,
                format!(
                    "burst slot {} provenance fingerprint {} did not match response fingerprint {}",
                    index,
                    fingerprint_hex(burst.provenance.config_fingerprint),
                    fingerprint_hex(response.config_fingerprint)
                ),
            ));
        }
        if burst.provenance.slot != index as u32 {
            return Err(ClientError::new(
                ClientErrorKind::DataLoss,
                format!(
                    "burst at response index {} reported provenance slot {}",
                    index, burst.provenance.slot
                ),
            ));
        }
    }

    Ok(())
}

pub fn propose_bursts_with_fingerprint_guard<C: InputSynthClient>(
    client: &mut C,
    bringup: &SynthBringup,
    registry: &mut FingerprintRegistry,
    request: ProposeBurstsRequest,
    fingerprint_retry_budget: u32,
) -> ClientResult<ProposeBurstsResponse> {
    validate_propose_bursts_request(&request)?;
    let profile = SynthProfile::from_request(&request);
    let mut remaining_fingerprint_retries = fingerprint_retry_budget;

    loop {
        let response = client.propose_bursts(request.clone())?;
        validate_propose_bursts_response(&request, &response)?;

        match registry.check_or_insert(profile.clone(), response.config_fingerprint) {
            Ok(_) => return Ok(response),
            Err(_) if remaining_fingerprint_retries > 0 => {
                remaining_fingerprint_retries -= 1;
                bringup.run(client)?;
                continue;
            }
            Err(mismatch) => return Err(mismatch.into_client_error()),
        }
    }
}

fn runtime() -> ClientResult<Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|error| {
            ClientError::new(
                ClientErrorKind::Internal,
                format!("failed to create input-synth runtime: {error}"),
            )
        })
}

fn request_with_deadline<T>(message: T, deadline: Duration) -> Request<T> {
    let mut request = Request::new(message);
    request.set_timeout(deadline);
    request
}

fn transport_error(error: tonic::transport::Error) -> ClientError {
    ClientError::new(
        ClientErrorKind::Unavailable,
        format!("input-synth transport unavailable: {error}"),
    )
}

fn dto_to_wire_node_context(context: &NodeContext) -> ClientResult<wire::NodeContext> {
    if !context.frame_embedding.is_empty() {
        return Err(ClientError::new(
            ClientErrorKind::InvalidRequest,
            "node_context.frame_embedding must stay empty until Phase 8 defines f64-to-f32 policy",
        ));
    }

    Ok(wire::NodeContext {
        node_id: node_id_to_wire(context.node_id),
        snapshot_ref: snapshot_ref_to_wire(context.snapshot_ref),
        depth: context.depth,
        node_score: context.node_score.get(),
        novelty: context.novelty.get(),
        ram_features: context
            .ram_features
            .iter()
            .map(|(key, value)| (key.clone(), value.get()))
            .collect(),
        frame_embedding: Vec::new(),
        recent_inputs: context
            .recent_inputs
            .as_ref()
            .map(dto_to_wire_burst)
            .transpose()?,
        parent_burst: context
            .parent_burst
            .as_ref()
            .map(dto_to_wire_provenanced_burst)
            .transpose()?,
        sibling_bursts: context
            .sibling_bursts
            .iter()
            .map(dto_to_wire_scored_burst)
            .collect::<ClientResult<_>>()?,
    })
}

fn dto_to_wire_scored_burst(scored: &ScoredBurst) -> ClientResult<wire::ScoredBurst> {
    Ok(wire::ScoredBurst {
        burst: Some(dto_to_wire_provenanced_burst(&scored.burst)?),
        score_delta: scored.score_delta.get(),
    })
}

pub fn wire_to_dto_scored_burst(scored: wire::ScoredBurst) -> ClientResult<ScoredBurst> {
    Ok(ScoredBurst {
        burst: wire_to_dto_provenanced_burst(scored.burst.ok_or_else(|| {
            ClientError::new(ClientErrorKind::DataLoss, "ScoredBurst.burst is missing")
        })?)?,
        score_delta: finite_f64_response(scored.score_delta, "ScoredBurst.score_delta")?,
    })
}

fn dto_to_wire_provenanced_burst(burst: &ProvenancedBurst) -> ClientResult<wire::ProvenancedBurst> {
    Ok(wire::ProvenancedBurst {
        burst: Some(dto_to_wire_burst(&burst.burst)?),
        provenance: Some(dto_to_wire_provenance(&burst.provenance)?),
    })
}

fn wire_to_dto_provenanced_burst(burst: wire::ProvenancedBurst) -> ClientResult<ProvenancedBurst> {
    Ok(ProvenancedBurst {
        burst: wire_to_dto_burst(burst.burst.ok_or_else(|| {
            ClientError::new(
                ClientErrorKind::DataLoss,
                "ProvenancedBurst.burst is missing",
            )
        })?)?,
        provenance: wire_to_dto_provenance(burst.provenance.ok_or_else(|| {
            ClientError::new(
                ClientErrorKind::DataLoss,
                "ProvenancedBurst.provenance is missing",
            )
        })?)?,
    })
}

fn dto_to_wire_burst(burst: &Burst) -> ClientResult<wire::Burst> {
    Ok(wire::Burst {
        format_version: burst.format_version,
        burst_id: burst.burst_id.as_bytes().to_vec(),
        body: Some(match &burst.body {
            BurstBody::Pad(pad) => wire::burst::Body::Pad(dto_to_wire_pad_burst(pad)),
            BurstBody::Event(event) => wire::burst::Body::Event(dto_to_wire_event_burst(event)?),
        }),
    })
}

fn wire_to_dto_burst(burst: wire::Burst) -> ClientResult<Burst> {
    if burst.format_version != wire::BURST_FORMAT_VERSION {
        return Err(ClientError::new(
            ClientErrorKind::DataLoss,
            format!(
                "burst format_version {} did not match expected {}",
                burst.format_version,
                wire::BURST_FORMAT_VERSION
            ),
        ));
    }
    Ok(Burst {
        format_version: burst.format_version,
        burst_id: BurstId::new(fixed_bytes::<BURST_ID_LEN>(
            &burst.burst_id,
            ClientErrorKind::DataLoss,
            "Burst.burst_id",
        )?),
        body: match burst.body.ok_or_else(|| {
            ClientError::new(ClientErrorKind::DataLoss, "Burst.body oneof is unset")
        })? {
            wire::burst::Body::Pad(pad) => BurstBody::Pad(wire_to_dto_pad_burst(pad)),
            wire::burst::Body::Event(event) => BurstBody::Event(wire_to_dto_event_burst(event)?),
        },
    })
}

fn dto_to_wire_pad_burst(pad: &PadBurst) -> wire::PadBurst {
    wire::PadBurst {
        segments: pad
            .segments
            .iter()
            .map(|segment| wire::PadSegment {
                buttons: segment.buttons,
                hold_frames: segment.hold_frames.get(),
            })
            .collect(),
        button_alphabet: pad.button_alphabet.clone(),
    }
}

fn wire_to_dto_pad_burst(pad: wire::PadBurst) -> PadBurst {
    PadBurst {
        segments: pad
            .segments
            .into_iter()
            .map(|segment| PadSegment {
                buttons: segment.buttons,
                hold_frames: FrameCount::new(segment.hold_frames),
            })
            .collect(),
        button_alphabet: pad.button_alphabet,
    }
}

fn dto_to_wire_event_burst(event: &EventBurst) -> ClientResult<wire::EventBurst> {
    Ok(wire::EventBurst {
        events: event
            .events
            .iter()
            .map(dto_to_wire_grammar_event)
            .collect::<ClientResult<_>>()?,
        grammar_id: event.grammar_id.clone(),
    })
}

fn wire_to_dto_event_burst(event: wire::EventBurst) -> ClientResult<EventBurst> {
    Ok(EventBurst {
        events: event
            .events
            .into_iter()
            .map(wire_to_dto_grammar_event)
            .collect::<ClientResult<_>>()?,
        grammar_id: event.grammar_id,
    })
}

fn dto_to_wire_grammar_event(event: &GrammarEvent) -> ClientResult<wire::GrammarEvent> {
    Ok(wire::GrammarEvent {
        event_type: event.event_type.clone(),
        at_offset_ns: event.at_offset_ns,
        fields: event
            .fields
            .iter()
            .map(dto_to_wire_grammar_field)
            .collect::<ClientResult<_>>()?,
        payload: event.payload.clone(),
    })
}

fn wire_to_dto_grammar_event(event: wire::GrammarEvent) -> ClientResult<GrammarEvent> {
    Ok(GrammarEvent {
        event_type: event.event_type,
        at_offset_ns: event.at_offset_ns,
        fields: event
            .fields
            .into_iter()
            .map(wire_to_dto_grammar_field)
            .collect::<ClientResult<_>>()?,
        payload: event.payload,
    })
}

fn dto_to_wire_grammar_field(field: &GrammarField) -> ClientResult<wire::GrammarField> {
    Ok(wire::GrammarField {
        name: field.name.clone(),
        value: Some(dto_to_wire_field_value(&field.value)),
    })
}

fn wire_to_dto_grammar_field(field: wire::GrammarField) -> ClientResult<GrammarField> {
    Ok(GrammarField {
        name: field.name,
        value: wire_to_dto_field_value(field.value.ok_or_else(|| {
            ClientError::new(ClientErrorKind::DataLoss, "GrammarField.value is missing")
        })?)?,
    })
}

fn dto_to_wire_field_value(value: &FieldValue) -> wire::FieldValue {
    wire::FieldValue {
        value: Some(match value {
            FieldValue::Int(value) => wire::field_value::Value::IntVal(*value),
            FieldValue::Enum(value) => wire::field_value::Value::EnumVal(value.clone()),
            FieldValue::DurationNs(value) => wire::field_value::Value::DurNs(*value),
            FieldValue::Bytes(value) => wire::field_value::Value::BytesVal(value.clone()),
        }),
    }
}

fn wire_to_dto_field_value(value: wire::FieldValue) -> ClientResult<FieldValue> {
    match value.value.ok_or_else(|| {
        ClientError::new(ClientErrorKind::DataLoss, "FieldValue.value oneof is unset")
    })? {
        wire::field_value::Value::IntVal(value) => Ok(FieldValue::Int(value)),
        wire::field_value::Value::EnumVal(value) => Ok(FieldValue::Enum(value)),
        wire::field_value::Value::DurNs(value) => Ok(FieldValue::DurationNs(value)),
        wire::field_value::Value::BytesVal(value) => Ok(FieldValue::Bytes(value)),
    }
}

fn dto_to_wire_provenance(provenance: &Provenance) -> ClientResult<wire::Provenance> {
    Ok(wire::Provenance {
        generator: dto_generator_kind_to_wire(provenance.generator) as i32,
        slot: provenance.slot,
        rng_stream: provenance.rng_stream.clone(),
        config_fingerprint: provenance.config_fingerprint.as_bytes().to_vec(),
        fallback_from: provenance
            .fallback_from
            .map(dto_generator_kind_to_wire)
            .unwrap_or(wire::GeneratorKind::Unspecified) as i32,
        r#macro: provenance
            .macro_provenance
            .as_ref()
            .map(dto_to_wire_macro_provenance),
        mutation: provenance
            .mutation_provenance
            .as_ref()
            .map(dto_to_wire_mutation_provenance)
            .transpose()?,
        policy: provenance
            .policy_provenance
            .as_ref()
            .map(dto_to_wire_policy_provenance),
    })
}

fn wire_to_dto_provenance(provenance: wire::Provenance) -> ClientResult<Provenance> {
    Ok(Provenance {
        generator: wire_generator_kind_to_dto_required(provenance.generator)?,
        slot: provenance.slot,
        rng_stream: provenance.rng_stream,
        config_fingerprint: config_fingerprint_from_wire(&provenance.config_fingerprint)?,
        fallback_from: wire_generator_kind_to_dto_optional(provenance.fallback_from)?,
        macro_provenance: provenance.r#macro.map(wire_to_dto_macro_provenance),
        mutation_provenance: provenance
            .mutation
            .map(wire_to_dto_mutation_provenance)
            .transpose()?,
        policy_provenance: provenance
            .policy
            .map(wire_to_dto_policy_provenance)
            .transpose()?,
    })
}

fn dto_to_wire_macro_provenance(provenance: &MacroProvenance) -> wire::MacroProvenance {
    wire::MacroProvenance {
        pack_id: provenance.pack_id.clone(),
        macro_name: provenance.macro_name.clone(),
        param_bindings: provenance.param_bindings.clone().into_iter().collect(),
        macro_frames: provenance.macro_frames.get(),
        tail_frames: provenance.tail_frames.get(),
        chain_index: provenance.chain_index,
    }
}

fn wire_to_dto_macro_provenance(provenance: wire::MacroProvenance) -> MacroProvenance {
    MacroProvenance {
        pack_id: provenance.pack_id,
        macro_name: provenance.macro_name,
        param_bindings: provenance.param_bindings.into_iter().collect(),
        macro_frames: FrameCount::new(provenance.macro_frames),
        tail_frames: FrameCount::new(provenance.tail_frames),
        chain_index: provenance.chain_index,
    }
}

fn dto_to_wire_mutation_provenance(
    provenance: &MutationProvenance,
) -> ClientResult<wire::MutationProvenance> {
    validate_mutation_ids(
        &provenance.base_burst_id,
        &provenance.donor_burst_id,
        ClientErrorKind::InvalidRequest,
    )?;
    Ok(wire::MutationProvenance {
        base_burst_id: provenance.base_burst_id.clone(),
        donor_burst_id: provenance.donor_burst_id.clone(),
        base_was_sibling: provenance.base_was_sibling,
        ops: provenance.ops.iter().map(dto_to_wire_mutation_op).collect(),
        post_clamp: provenance.post_clamp,
    })
}

fn wire_to_dto_mutation_provenance(
    provenance: wire::MutationProvenance,
) -> ClientResult<MutationProvenance> {
    validate_mutation_ids(
        &provenance.base_burst_id,
        &provenance.donor_burst_id,
        ClientErrorKind::DataLoss,
    )?;
    Ok(MutationProvenance {
        base_burst_id: provenance.base_burst_id,
        donor_burst_id: provenance.donor_burst_id,
        base_was_sibling: provenance.base_was_sibling,
        ops: provenance
            .ops
            .into_iter()
            .map(wire_to_dto_mutation_op)
            .collect(),
        post_clamp: provenance.post_clamp,
    })
}

fn dto_to_wire_mutation_op(op: &MutationOp) -> wire::MutationOp {
    wire::MutationOp {
        op: op.op.clone(),
        args: op.args.clone().into_iter().collect(),
    }
}

fn wire_to_dto_mutation_op(op: wire::MutationOp) -> MutationOp {
    MutationOp {
        op: op.op,
        args: op.args.into_iter().collect(),
    }
}

fn dto_to_wire_policy_provenance(provenance: &PolicyProvenance) -> wire::PolicyProvenance {
    wire::PolicyProvenance {
        model_id: provenance.model_id.clone(),
        model_version: provenance.model_version.clone(),
        temperature: provenance.temperature.get(),
        server_attested_deterministic: provenance.server_attested_deterministic,
    }
}

fn wire_to_dto_policy_provenance(
    provenance: wire::PolicyProvenance,
) -> ClientResult<PolicyProvenance> {
    Ok(PolicyProvenance {
        model_id: provenance.model_id,
        model_version: provenance.model_version,
        temperature: finite_f64_response(provenance.temperature, "PolicyProvenance.temperature")?,
        server_attested_deterministic: provenance.server_attested_deterministic,
    })
}

fn dto_to_wire_path_sample(sample: &PathSample) -> ClientResult<wire::PathSample> {
    Ok(wire::PathSample {
        expansions: sample
            .expansions
            .iter()
            .map(dto_to_wire_scored_burst)
            .collect::<ClientResult<_>>()?,
        terminal_score: sample.terminal_score.get(),
    })
}

fn dto_to_wire_mining_params(params: &MiningParams) -> ClientResult<wire::MiningParams> {
    Ok(wire::MiningParams {
        min_support: params.min_support,
        min_paths: params.min_paths,
        max_len_tokens: params.max_len_tokens,
        max_macros: params.max_macros,
        containment_alpha: params.containment_alpha.map(FiniteF64::get),
        dedup_edit_dist: params.dedup_edit_dist.map(FiniteF64::get),
    })
}

fn wire_to_dto_mined_macro_stats(stats: wire::MinedMacroStats) -> ClientResult<MinedMacroStats> {
    Ok(MinedMacroStats {
        name: stats.name,
        support: stats.support,
        paths: stats.paths,
        lift: finite_f64_response(stats.lift, "MinedMacroStats.lift")?,
        score: score_response(stats.score, "MinedMacroStats.score")?,
        len_tokens: stats.len_tokens,
    })
}

fn wire_to_dto_degraded_generator(
    degraded: wire::DegradedGenerator,
) -> ClientResult<DegradedGenerator> {
    Ok(DegradedGenerator {
        generator: wire_generator_kind_to_dto_required(degraded.generator)?,
        reason: degraded.reason,
    })
}

fn dto_document_kind_to_wire(kind: DocumentKind) -> wire::DocumentKind {
    match kind {
        DocumentKind::MacroPack => wire::DocumentKind::MacroPack,
        DocumentKind::ExperimentConfig => wire::DocumentKind::ExperimentConfig,
        DocumentKind::EventGrammar => wire::DocumentKind::EventGrammar,
    }
}

fn dto_model_kind_to_wire(kind: ModelKind) -> wire::ModelKind {
    match kind {
        ModelKind::Pad => wire::ModelKind::Pad,
        ModelKind::EventGrammar => wire::ModelKind::EventGrammar,
    }
}

fn dto_generator_kind_to_wire(kind: GeneratorKind) -> wire::GeneratorKind {
    match kind {
        GeneratorKind::WeightedRandom => wire::GeneratorKind::WeightedRandom,
        GeneratorKind::Macro => wire::GeneratorKind::Macro,
        GeneratorKind::Mutation => wire::GeneratorKind::Mutation,
        GeneratorKind::Policy => wire::GeneratorKind::Policy,
    }
}

fn wire_health_status_to_dto(status: i32) -> ClientResult<HealthStatus> {
    match wire::health_response::Status::try_from(status) {
        Ok(wire::health_response::Status::Serving) => Ok(HealthStatus::Serving),
        Ok(wire::health_response::Status::Degraded) => Ok(HealthStatus::Degraded),
        Ok(wire::health_response::Status::NotServing) => Ok(HealthStatus::NotServing),
        Ok(wire::health_response::Status::Unspecified) => Err(ClientError::new(
            ClientErrorKind::DataLoss,
            "HealthResponse.status is unspecified",
        )),
        Err(_) => Err(ClientError::new(
            ClientErrorKind::DataLoss,
            format!("unknown HealthResponse.status enum value {status}"),
        )),
    }
}

fn wire_generator_kind_to_dto_required(value: i32) -> ClientResult<GeneratorKind> {
    match wire::GeneratorKind::try_from(value) {
        Ok(wire::GeneratorKind::WeightedRandom) => Ok(GeneratorKind::WeightedRandom),
        Ok(wire::GeneratorKind::Macro) => Ok(GeneratorKind::Macro),
        Ok(wire::GeneratorKind::Mutation) => Ok(GeneratorKind::Mutation),
        Ok(wire::GeneratorKind::Policy) => Ok(GeneratorKind::Policy),
        Ok(wire::GeneratorKind::Unspecified) => Err(ClientError::new(
            ClientErrorKind::DataLoss,
            "GeneratorKind is unspecified",
        )),
        Err(_) => Err(ClientError::new(
            ClientErrorKind::DataLoss,
            format!("unknown GeneratorKind enum value {value}"),
        )),
    }
}

fn wire_generator_kind_to_dto_optional(value: i32) -> ClientResult<Option<GeneratorKind>> {
    match wire::GeneratorKind::try_from(value) {
        Ok(wire::GeneratorKind::Unspecified) => Ok(None),
        Ok(wire::GeneratorKind::WeightedRandom) => Ok(Some(GeneratorKind::WeightedRandom)),
        Ok(wire::GeneratorKind::Macro) => Ok(Some(GeneratorKind::Macro)),
        Ok(wire::GeneratorKind::Mutation) => Ok(Some(GeneratorKind::Mutation)),
        Ok(wire::GeneratorKind::Policy) => Ok(Some(GeneratorKind::Policy)),
        Err(_) => Err(ClientError::new(
            ClientErrorKind::DataLoss,
            format!("unknown fallback GeneratorKind enum value {value}"),
        )),
    }
}

fn config_fingerprint_from_wire(bytes: &[u8]) -> ClientResult<ConfigFingerprint> {
    Ok(ConfigFingerprint::new(
        fixed_bytes::<CONFIG_FINGERPRINT_LEN>(
            bytes,
            ClientErrorKind::DataLoss,
            "ConfigFingerprint",
        )?,
    ))
}

fn validate_mutation_ids(base: &[u8], donor: &[u8], kind: ClientErrorKind) -> ClientResult<()> {
    fixed_bytes::<BURST_ID_LEN>(base, kind, "MutationProvenance.base_burst_id")?;
    if !donor.is_empty() {
        fixed_bytes::<BURST_ID_LEN>(donor, kind, "MutationProvenance.donor_burst_id")?;
    }
    Ok(())
}

fn fixed_bytes<const N: usize>(
    bytes: &[u8],
    kind: ClientErrorKind,
    field: &str,
) -> ClientResult<[u8; N]> {
    bytes.try_into().map_err(|_| {
        ClientError::new(
            kind,
            format!("{field} must be exactly {N} bytes, got {}", bytes.len()),
        )
    })
}

fn finite_f64_response(value: f64, field: &str) -> ClientResult<FiniteF64> {
    FiniteF64::new(value).map_err(|error| {
        ClientError::new(
            ClientErrorKind::DataLoss,
            format!("{field} must be finite: {error}"),
        )
    })
}

fn score_response(value: f64, field: &str) -> ClientResult<Score> {
    Score::new(value).map_err(|error| {
        ClientError::new(
            ClientErrorKind::DataLoss,
            format!("{field} must be finite: {error}"),
        )
    })
}

fn _novelty_response(value: f64, field: &str) -> ClientResult<Novelty> {
    Novelty::new(value).map_err(|error| {
        ClientError::new(
            ClientErrorKind::DataLoss,
            format!("{field} must be finite: {error}"),
        )
    })
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

fn decode_fixed_hex<const N: usize>(
    value: &str,
    kind: ClientErrorKind,
    field: &str,
) -> ClientResult<[u8; N]> {
    if value.len() != N * 2 {
        return Err(ClientError::new(
            kind,
            format!(
                "{field} hex must be {} characters, got {}",
                N * 2,
                value.len()
            ),
        ));
    }

    let mut bytes = [0u8; N];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_nibble(chunk[0]).ok_or_else(|| {
            ClientError::new(kind, format!("{field} contains invalid hex character"))
        })?;
        let low = hex_nibble(chunk[1]).ok_or_else(|| {
            ClientError::new(kind, format!("{field} contains invalid hex character"))
        })?;
        bytes[index] = (high << 4) | low;
    }
    Ok(bytes)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn parse_required_macro_pack_ids(config_yaml: &[u8]) -> ClientResult<BTreeSet<String>> {
    let text = std::str::from_utf8(config_yaml).map_err(|_| {
        ClientError::new(
            ClientErrorKind::InvalidRequest,
            "experiment synth config is not valid UTF-8",
        )
    })?;
    let mut ids = BTreeSet::new();
    let mut in_macro = false;
    let mut macro_indent = 0usize;
    let mut in_packs = false;
    let mut packs_indent = 0usize;

    for raw_line in text.lines() {
        let line = strip_comment(raw_line);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();

        if in_packs && indent <= packs_indent && !trimmed.starts_with('-') {
            in_packs = false;
        }
        if in_macro && indent <= macro_indent && !trimmed.starts_with("packs:") {
            in_macro = false;
        }

        if let Some(rest) = trimmed.strip_prefix("macro.packs:") {
            parse_pack_value(rest, &mut ids)?;
            in_packs = rest.trim().is_empty();
            packs_indent = indent;
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("macro:") {
            in_macro = true;
            macro_indent = indent;
            if rest.trim().starts_with('{') && rest.contains("packs") {
                parse_inline_object_packs(rest, &mut ids)?;
            }
            continue;
        }

        if in_macro {
            if let Some(rest) = trimmed.strip_prefix("packs:") {
                parse_pack_value(rest, &mut ids)?;
                in_packs = rest.trim().is_empty();
                packs_indent = indent;
                continue;
            }
        }

        if in_packs {
            if let Some(rest) = trimmed.strip_prefix('-') {
                let item = parse_pack_item(rest.trim())?;
                if !item.is_empty() {
                    ids.insert(item);
                }
            }
        }
    }

    Ok(ids)
}

fn parse_pack_value(rest: &str, ids: &mut BTreeSet<String>) -> ClientResult<()> {
    let value = rest.trim();
    if value.is_empty() {
        return Ok(());
    }
    if value.starts_with('[') {
        let Some(inner) = value
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
        else {
            return Err(ClientError::new(
                ClientErrorKind::InvalidRequest,
                "macro.packs inline list must close with ']'",
            ));
        };
        for item in inner.split(',') {
            let item = parse_pack_item(item.trim())?;
            if !item.is_empty() {
                ids.insert(item);
            }
        }
        return Ok(());
    }

    let item = parse_pack_item(value)?;
    if !item.is_empty() {
        ids.insert(item);
    }
    Ok(())
}

fn parse_inline_object_packs(rest: &str, ids: &mut BTreeSet<String>) -> ClientResult<()> {
    let Some(packs_index) = rest.find("packs") else {
        return Ok(());
    };
    let Some(colon_index) = rest[packs_index..].find(':') else {
        return Ok(());
    };
    let value = &rest[packs_index + colon_index + 1..];
    let Some(list_start) = value.find('[') else {
        return Ok(());
    };
    let Some(list_end) = value[list_start..].find(']') else {
        return Err(ClientError::new(
            ClientErrorKind::InvalidRequest,
            "macro.packs inline object list must close with ']'",
        ));
    };
    parse_pack_value(&value[list_start..=list_start + list_end], ids)
}

fn parse_pack_item(value: &str) -> ClientResult<String> {
    let trimmed = value.trim().trim_end_matches(',');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(trimmed);
    if unquoted.trim().is_empty() {
        return Err(ClientError::new(
            ClientErrorKind::InvalidRequest,
            "macro.packs contains an empty pack id",
        ));
    }
    Ok(unquoted.trim().to_owned())
}

fn strip_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or(line).trim_end()
}

fn fingerprint_hex(fingerprint: ConfigFingerprint) -> String {
    let mut out = String::with_capacity(64);
    for byte in fingerprint.as_bytes() {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_clients::input_synth::{
        Burst, BurstBody, BurstId, PadBurst, PadSegment, Provenance, ProvenancedBurst,
    };
    use orch_core::types::FiniteF64;

    #[test]
    fn parses_required_macro_packs_from_inline_and_block_forms() {
        let parsed = parse_required_macro_pack_ids(
            br#"
macro:
  packs: [ console16-movement-core, "boss-pack" ]
other: ignored
macro.packs:
  - extra-pack
  - 'late-pack'
"#,
        )
        .expect("parse packs");

        assert_eq!(
            parsed,
            BTreeSet::from([
                "boss-pack".to_owned(),
                "console16-movement-core".to_owned(),
                "extra-pack".to_owned(),
                "late-pack".to_owned()
            ])
        );
    }

    #[test]
    fn validates_response_before_fingerprint_registry_insert() {
        let request = sample_request();
        let registry = FingerprintRegistry::new();
        let mut response = sample_response(ConfigFingerprint::new([0xA5; 32]));
        response.seed = request.seed + 1;

        let error = validate_propose_bursts_response(&request, &response)
            .expect_err("seed mismatch is data loss");
        assert_eq!(error.kind(), ClientErrorKind::DataLoss);
        assert!(registry
            .expected_fingerprint(&SynthProfile::from_request(&request))
            .is_none());
    }

    #[test]
    fn fingerprint_registry_mismatch_does_not_replace_expected_value() {
        let request = sample_request();
        let profile = SynthProfile::from_request(&request);
        let mut registry = FingerprintRegistry::new();
        let expected = ConfigFingerprint::new([0xA5; 32]);
        let actual = ConfigFingerprint::new([0x5A; 32]);

        assert_eq!(
            registry.check_or_insert(profile.clone(), expected),
            Ok(FingerprintCheck::Inserted)
        );
        let mismatch = registry
            .check_or_insert(profile.clone(), actual)
            .expect_err("mismatch");

        assert_eq!(mismatch.expected, expected);
        assert_eq!(mismatch.actual, actual);
        assert_eq!(registry.expected_fingerprint(&profile), Some(expected));
    }

    #[test]
    fn rejects_phase8_frame_embedding_until_conversion_policy_exists() {
        let mut request = sample_request();
        request.node_context.frame_embedding = vec![FiniteF64::new(1.0).expect("finite")];

        let error =
            validate_propose_bursts_request(&request).expect_err("frame embedding rejected");

        assert_eq!(error.kind(), ClientErrorKind::InvalidRequest);
    }

    fn sample_request() -> ProposeBurstsRequest {
        ProposeBurstsRequest {
            experiment_id: "exp-a".to_owned(),
            node_context: orch_clients::input_synth::NodeContext {
                node_id: NodeId::new(1),
                parent_node_id: Some(NodeId::ROOT),
                snapshot_ref: orch_core::types::SnapshotRef::new([1; 32]),
                state_hash: orch_core::types::StateHash::new([2; 32]),
                cell_key: orch_core::types::CellKey::new(3),
                stage: orch_core::types::Stage::new(4),
                depth: 1,
                frame_counter: FrameCount::new(5),
                node_score: orch_core::types::Score::new(6.0).expect("finite"),
                novelty: orch_core::types::Novelty::new(0.25).expect("finite"),
                ram_features: BTreeMap::new(),
                frame_embedding: Vec::new(),
                recent_inputs: None,
                parent_burst: None,
                sibling_bursts: Vec::new(),
            },
            k: 1,
            length_hint: FrameCount::new(8),
            seed: 99,
            model: ModelKind::Pad,
            config_overrides_yaml: Vec::new(),
        }
    }

    fn sample_response(fingerprint: ConfigFingerprint) -> ProposeBurstsResponse {
        ProposeBurstsResponse {
            bursts: vec![ProvenancedBurst {
                burst: Burst {
                    format_version: 1,
                    burst_id: BurstId::new([9; 32]),
                    body: BurstBody::Pad(PadBurst {
                        segments: vec![PadSegment {
                            buttons: 1,
                            hold_frames: FrameCount::new(2),
                        }],
                        button_alphabet: "console16-12btn-v1".to_owned(),
                    }),
                },
                provenance: Provenance {
                    generator: orch_clients::input_synth::GeneratorKind::WeightedRandom,
                    slot: 0,
                    rng_stream: "test".to_owned(),
                    config_fingerprint: fingerprint,
                    fallback_from: None,
                    macro_provenance: None,
                    mutation_provenance: None,
                    policy_provenance: None,
                },
            }],
            config_fingerprint: fingerprint,
            synth_version: "test".to_owned(),
            seed: 99,
            degraded: Vec::new(),
        }
    }
}
