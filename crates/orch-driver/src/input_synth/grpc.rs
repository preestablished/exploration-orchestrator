//! gRPC transport adapter for the input synthesizer: the generated tonic
//! client plus DTO<->wire conversions. Compiled only with the `grpc` cargo
//! feature so transport-free consumers of `orch-driver` never link tonic.

use std::{sync::Mutex, time::Duration};

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
    ClientError, ClientErrorKind, ClientResult,
};
use orch_core::types::{FiniteF64, FrameCount, NodeId, Score, SnapshotRef, DIGEST_LEN};
use orch_proto::inputsynth::v1 as wire;
use tokio::runtime::Runtime;
use tonic::{
    transport::{Channel, Endpoint},
    Code, Request, Status,
};

use super::{validate_propose_bursts_request, validate_propose_bursts_response};

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
            let result = self
                .runtime
                .block_on(client.health(request_with_deadline(wire_request, self.config.deadline)));
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
        Code::DataLoss => ClientErrorKind::DataLoss,
        Code::Internal | Code::Unknown => ClientErrorKind::Internal,
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

fn runtime() -> ClientResult<Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
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
        body: Some(match &burst.body {
            BurstBody::Pad(pad) => wire::burst::Body::Pad(dto_to_wire_pad_burst(pad)),
            BurstBody::Event(event) => wire::burst::Body::Event(dto_to_wire_event_burst(event)?),
        }),
        burst_id: burst.burst_id.as_bytes().to_vec(),
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
    validate_provenance_payloads(provenance, ClientErrorKind::InvalidRequest)?;

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
    let dto = Provenance {
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
    };
    validate_provenance_payloads(&dto, ClientErrorKind::DataLoss)?;
    Ok(dto)
}

fn validate_provenance_payloads(
    provenance: &Provenance,
    kind: ClientErrorKind,
) -> ClientResult<()> {
    let macro_set = provenance.macro_provenance.is_some();
    let mutation_set = provenance.mutation_provenance.is_some();
    let policy_set = provenance.policy_provenance.is_some();

    let expected_payload = match provenance.generator {
        GeneratorKind::WeightedRandom => None,
        GeneratorKind::Macro => Some("macro"),
        GeneratorKind::Mutation => Some("mutation"),
        GeneratorKind::Policy => Some("policy"),
    };
    let actual_count = usize::from(macro_set) + usize::from(mutation_set) + usize::from(policy_set);

    if expected_payload.is_none() {
        if actual_count == 0 {
            return Ok(());
        }
        return Err(ClientError::new(
            kind,
            "weighted-random provenance must not include generator-specific payloads",
        ));
    }

    if actual_count != 1 {
        return Err(ClientError::new(
            kind,
            format!(
                "{:?} provenance must include exactly one matching generator payload",
                provenance.generator
            ),
        ));
    }

    let payload_matches = match provenance.generator {
        GeneratorKind::WeightedRandom => true,
        GeneratorKind::Macro => macro_set,
        GeneratorKind::Mutation => mutation_set,
        GeneratorKind::Policy => policy_set,
    };

    if payload_matches {
        Ok(())
    } else {
        Err(ClientError::new(
            kind,
            format!(
                "{:?} provenance must include {} payload only",
                provenance.generator,
                expected_payload.expect("non-weighted generators have payloads")
            ),
        ))
    }
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
    // Wire MiningParams fields are plain proto3 scalars: zero means "use the
    // synthesizer-side default", matching the DTO's `None`.
    Ok(wire::MiningParams {
        min_support: params.min_support.unwrap_or(0),
        min_paths: params.min_paths.unwrap_or(0),
        max_len_tokens: params.max_len_tokens.unwrap_or(0),
        max_macros: params.max_macros.unwrap_or(0),
        containment_alpha: params.containment_alpha.map_or(0.0, FiniteF64::get),
        dedup_edit_dist: params.dedup_edit_dist.map_or(0.0, FiniteF64::get),
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
