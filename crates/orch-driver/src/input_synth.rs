//! Driver-owned input-synth bring-up, request, and fingerprint helpers.
//!
//! Everything in this module is transport-free and generic over the
//! [`InputSynthClient`] trait. The tonic-backed adapter and its DTO<->wire
//! conversions live in the [`grpc`] submodule behind the `grpc` cargo feature.

use std::collections::{BTreeMap, BTreeSet};

use orch_clients::{
    input_synth::{
        ConfigFingerprint, DocumentKind, HealthRequest, HealthResponse, HealthStatus,
        InputSynthClient, LoadMacroPackRequest, LoadMacroPackResponse, LoadMacroPackSource,
        ModelKind, ProposeBurstsRequest, ProposeBurstsResponse,
    },
    snapshot_store::SnapshotStoreClient,
    ClientError, ClientErrorKind, ClientResult,
};
use orch_core::{
    rng::derive_synth_request_seed,
    types::{FrameCount, NodeId, Novelty},
};

use crate::node_attrs::{build_input_synth_node_context, NodeContextLimits};

#[cfg(feature = "grpc")]
mod grpc;
#[cfg(feature = "grpc")]
pub use grpc::*;

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
        if !health
            .loaded_experiments
            .iter()
            .any(|experiment_id| experiment_id == &self.experiment_id)
        {
            return Err(ClientError::new(
                ClientErrorKind::FailedPrecondition,
                format!(
                    "input synthesizer health is missing loaded experiment '{}'",
                    self.experiment_id
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

fn _novelty_response(value: f64, field: &str) -> ClientResult<Novelty> {
    Novelty::new(value).map_err(|error| {
        ClientError::new(
            ClientErrorKind::DataLoss,
            format!("{field} must be finite: {error}"),
        )
    })
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
