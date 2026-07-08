//! Fake input synthesizer surface for deterministic burst generation tests.
//!
//! This fake satisfies [`orch_clients::input_synth::InputSynthClient`] without
//! transport, filesystem, wall-clock, or policy-service dependencies.

use std::{cell::Cell, collections::BTreeMap, str};

use orch_clients::{
    input_synth::{
        Burst, BurstBody, BurstId, ConfigFingerprint, DegradedGenerator, DocumentKind, EventBurst,
        FieldValue, GeneratorKind, GrammarEvent, GrammarField, HealthRequest, HealthResponse,
        HealthStatus, InputSynthClient, LoadMacroPackRequest, LoadMacroPackResponse,
        LoadMacroPackSource, MacroProvenance, MineMacrosRequest, MineMacrosResponse,
        MinedMacroStats, ModelKind, MutationOp, MutationProvenance, PadBurst, PadSegment,
        PolicyProvenance, ProposeBurstsRequest, ProposeBurstsResponse, Provenance,
        ProvenancedBurst,
    },
    ClientError, ClientErrorKind, ClientResult,
};
use orch_core::{
    rng::DeterministicRng,
    types::{FiniteF64, FrameCount, Score},
};

use crate::fault::{
    FaultDecision, FaultInjector, FaultPlan, FaultRequest, FaultStats, FaultTarget,
};

pub const FAKE_SYNTH_VERSION: &str = "fake-synth/0.1";
pub const MAX_PROPOSE_BURSTS: u32 = 256;

const BUTTON_ALPHABET: &str = "console16-12btn-v1";
const BUTTON_MASKS: [u32; 8] = [
    0,
    0b0000_0001,
    0b0000_0010,
    0b0000_1000,
    0b0000_0011,
    0b0100_0000,
    0b1_0000_0000,
    0b10_0000_0000,
];

#[derive(Clone, Debug, PartialEq)]
pub struct FakeSynth {
    synth_version: String,
    loaded_packs: BTreeMap<String, FakeMacroPack>,
    loaded_experiments: BTreeMap<String, FakeExperimentConfig>,
    fault_injector: FaultInjector,
    last_fault: Cell<Option<FaultDecision>>,
}

impl Default for FakeSynth {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeSynth {
    #[must_use]
    pub fn new() -> Self {
        Self::with_fault_plan(FaultPlan::disabled(0))
    }

    #[must_use]
    pub fn with_fault_plan(fault_plan: FaultPlan) -> Self {
        Self {
            synth_version: FAKE_SYNTH_VERSION.to_owned(),
            loaded_packs: BTreeMap::new(),
            loaded_experiments: BTreeMap::new(),
            fault_injector: FaultInjector::new(fault_plan),
            last_fault: Cell::new(None),
        }
    }

    #[must_use]
    pub fn last_fault(&self) -> Option<FaultDecision> {
        self.last_fault.get()
    }

    #[must_use]
    pub fn fault_stats(&self) -> FaultStats {
        self.fault_injector.stats()
    }

    #[must_use]
    pub fn loaded_pack_ids(&self) -> Vec<String> {
        self.loaded_packs.keys().cloned().collect()
    }

    #[must_use]
    pub fn loaded_experiment_ids(&self) -> Vec<String> {
        self.loaded_experiments.keys().cloned().collect()
    }

    #[must_use]
    pub fn preview_config_fingerprint(
        &self,
        experiment_id: &str,
        overrides_yaml: &[u8],
    ) -> Option<ConfigFingerprint> {
        let experiment = self.loaded_experiments.get(experiment_id)?;
        let mix = experiment.mix.with_overrides(overrides_yaml).ok()?;
        Some(self.config_fingerprint(experiment, mix))
    }

    fn load_experiment_config(
        &mut self,
        source: LoadMacroPackSource,
    ) -> ClientResult<LoadMacroPackResponse> {
        let document = LoadedDocument::from_source(source, "experiment-config");
        let text = document.text()?;
        let experiment_id =
            parse_scalar(text, "experiment_id").unwrap_or_else(|| document.document_id.clone());
        let model = parse_model(text).unwrap_or(ModelKind::Pad);
        let mix = GeneratorMix::default().with_config_document(text.as_bytes())?;
        let config = FakeExperimentConfig {
            experiment_id: experiment_id.clone(),
            model,
            button_alphabet: parse_scalar(text, "button_alphabet")
                .unwrap_or_else(|| BUTTON_ALPHABET.to_owned()),
            mix,
        };

        self.loaded_experiments
            .insert(experiment_id.clone(), config);

        Ok(LoadMacroPackResponse {
            document_id: experiment_id,
            items_loaded: 1,
            warnings: document.warnings,
        })
    }

    fn load_macro_pack_document(
        &mut self,
        source: LoadMacroPackSource,
    ) -> ClientResult<LoadMacroPackResponse> {
        let document = LoadedDocument::from_source(source, "macro-pack");
        let text = document.text()?;
        let pack_id = parse_scalar(text, "name").unwrap_or_else(|| document.document_id.clone());
        let mut macros = parse_macro_names(text)
            .into_iter()
            .enumerate()
            .map(|(index, name)| FakeMacro {
                name,
                weight: 1.0 + index as f64,
            })
            .collect::<Vec<_>>();
        if macros.is_empty() {
            macros.push(FakeMacro {
                name: "default-dash".to_owned(),
                weight: 1.0,
            });
        }
        let items_loaded = macros.len() as u32;

        self.loaded_packs.insert(
            pack_id.clone(),
            FakeMacroPack {
                pack_id: pack_id.clone(),
                macros,
            },
        );

        Ok(LoadMacroPackResponse {
            document_id: pack_id,
            items_loaded,
            warnings: document.warnings,
        })
    }

    fn propose_pad_burst(
        &self,
        request: &ProposeBurstsRequest,
        slot: u32,
        generator: GeneratorKind,
        fallback_from: Option<GeneratorKind>,
        fingerprint: ConfigFingerprint,
    ) -> ProvenancedBurst {
        let mut rng = DeterministicRng::synth(request.seed, u64::from(slot));
        let target_frames = target_frames(request.length_hint);
        let (body, macro_provenance, mutation_provenance, policy_provenance) = match generator {
            GeneratorKind::Macro => {
                let (pack, selected_macro) = self.select_macro(&mut rng);
                let direction = if rng.next_unit_f64() < 0.5 {
                    "left"
                } else {
                    "right"
                };
                let segments = macro_segments(selected_macro, direction, target_frames);
                (
                    BurstBody::Pad(PadBurst {
                        segments,
                        button_alphabet: BUTTON_ALPHABET.to_owned(),
                    }),
                    Some(MacroProvenance {
                        pack_id: pack.pack_id.clone(),
                        macro_name: selected_macro.name.clone(),
                        param_bindings: BTreeMap::from([(
                            "direction".to_owned(),
                            direction.to_owned(),
                        )]),
                        macro_frames: FrameCount::new(target_frames.min(24)),
                        tail_frames: FrameCount::new(target_frames.saturating_sub(24)),
                        chain_index: 0,
                    }),
                    None,
                    None,
                )
            }
            GeneratorKind::Mutation => {
                let (body, mutation) = mutate_parent_burst(request, &mut rng, target_frames);
                (body, None, Some(mutation), None)
            }
            GeneratorKind::Policy => {
                let segments = weighted_random_segments(&mut rng, target_frames);
                (
                    BurstBody::Pad(PadBurst {
                        segments,
                        button_alphabet: BUTTON_ALPHABET.to_owned(),
                    }),
                    None,
                    None,
                    Some(PolicyProvenance {
                        model_id: "fake-policy".to_owned(),
                        model_version: self.synth_version.clone(),
                        temperature: FiniteF64::new(1.0).expect("finite"),
                        server_attested_deterministic: true,
                    }),
                )
            }
            GeneratorKind::WeightedRandom => {
                let segments = weighted_random_segments(&mut rng, target_frames);
                (
                    BurstBody::Pad(PadBurst {
                        segments,
                        button_alphabet: BUTTON_ALPHABET.to_owned(),
                    }),
                    None,
                    None,
                    None,
                )
            }
        };

        self.provenanced_burst(
            request.seed,
            slot,
            generator,
            fallback_from,
            fingerprint,
            body,
            macro_provenance,
            mutation_provenance,
            policy_provenance,
        )
    }

    fn propose_event_burst(
        &self,
        request: &ProposeBurstsRequest,
        slot: u32,
        generator: GeneratorKind,
        fallback_from: Option<GeneratorKind>,
        fingerprint: ConfigFingerprint,
    ) -> ProvenancedBurst {
        let mut rng = DeterministicRng::synth(request.seed, u64::from(slot));
        let event_count = request.length_hint.get().clamp(1, 8);
        let mut events = Vec::with_capacity(event_count as usize);
        for index in 0..event_count {
            let value = (rng.next_unit_f64() * 1_000.0).floor() as i64;
            events.push(GrammarEvent {
                event_type: "fake_event".to_owned(),
                at_offset_ns: u64::from(index) * 1_000_000,
                fields: vec![GrammarField {
                    name: "value".to_owned(),
                    value: FieldValue::Int(value),
                }],
                payload: request.seed.to_le_bytes().to_vec(),
            });
        }

        self.provenanced_burst(
            request.seed,
            slot,
            generator,
            fallback_from,
            fingerprint,
            BurstBody::Event(EventBurst {
                events,
                grammar_id: "fake-grammar".to_owned(),
            }),
            None,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn provenanced_burst(
        &self,
        seed: u64,
        slot: u32,
        generator: GeneratorKind,
        fallback_from: Option<GeneratorKind>,
        fingerprint: ConfigFingerprint,
        body: BurstBody,
        macro_provenance: Option<MacroProvenance>,
        mutation_provenance: Option<MutationProvenance>,
        policy_provenance: Option<PolicyProvenance>,
    ) -> ProvenancedBurst {
        let burst_id = burst_id(seed, slot, fingerprint, &body);
        ProvenancedBurst {
            burst: Burst {
                format_version: 1,
                burst_id,
                body,
            },
            provenance: Provenance {
                generator,
                slot,
                rng_stream: format!("synth/{seed}/slot/{slot}/{}", generator_tag(generator)),
                config_fingerprint: fingerprint,
                fallback_from,
                macro_provenance,
                mutation_provenance,
                policy_provenance,
            },
        }
    }

    fn select_macro(&self, rng: &mut DeterministicRng) -> (&FakeMacroPack, &FakeMacro) {
        let total_weight = self
            .loaded_packs
            .values()
            .flat_map(|pack| pack.macros.iter())
            .map(|item| item.weight)
            .sum::<f64>();
        let mut cursor = rng.next_unit_f64() * total_weight;

        for pack in self.loaded_packs.values() {
            for item in &pack.macros {
                if cursor < item.weight {
                    return (pack, item);
                }
                cursor -= item.weight;
            }
        }

        let pack = self
            .loaded_packs
            .values()
            .next()
            .expect("macro generator is selected only when macros are loaded");
        let item = pack
            .macros
            .first()
            .expect("loaded macro packs contain at least one macro");
        (pack, item)
    }

    fn config_fingerprint(
        &self,
        experiment: &FakeExperimentConfig,
        effective_mix: GeneratorMix,
    ) -> ConfigFingerprint {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"orch-fakes/synth-config/v1");
        update_len_prefixed(&mut hasher, self.synth_version.as_bytes());
        update_len_prefixed(&mut hasher, experiment.experiment_id.as_bytes());
        update_len_prefixed(&mut hasher, model_tag(experiment.model).as_bytes());
        update_len_prefixed(&mut hasher, experiment.button_alphabet.as_bytes());
        effective_mix.update_hash(&mut hasher);
        for (pack_id, pack) in &self.loaded_packs {
            update_len_prefixed(&mut hasher, pack_id.as_bytes());
            for item in &pack.macros {
                update_len_prefixed(&mut hasher, item.name.as_bytes());
                hasher.update(&item.weight.to_bits().to_le_bytes());
            }
        }
        ConfigFingerprint::new(*hasher.finalize().as_bytes())
    }
}

impl InputSynthClient for FakeSynth {
    fn load_macro_pack(
        &mut self,
        request: LoadMacroPackRequest,
    ) -> ClientResult<LoadMacroPackResponse> {
        match request.kind {
            DocumentKind::ExperimentConfig => self.load_experiment_config(request.source),
            DocumentKind::MacroPack => self.load_macro_pack_document(request.source),
            DocumentKind::EventGrammar => Ok(LoadMacroPackResponse {
                document_id: LoadedDocument::from_source(request.source, "event-grammar")
                    .document_id,
                items_loaded: 1,
                warnings: Vec::new(),
            }),
        }
    }

    fn health(&self, _request: HealthRequest) -> ClientResult<HealthResponse> {
        let status = if self.loaded_experiments.is_empty() {
            HealthStatus::NotServing
        } else if self.loaded_packs.is_empty() {
            HealthStatus::Degraded
        } else {
            HealthStatus::Serving
        };

        Ok(HealthResponse {
            status,
            synth_version: self.synth_version.clone(),
            loaded_packs: self.loaded_pack_ids(),
            loaded_experiments: self.loaded_experiment_ids(),
            policy_endpoint_up: false,
            policy_deterministic: true,
            mining_in_progress: false,
        })
    }

    fn propose_bursts(
        &mut self,
        request: ProposeBurstsRequest,
    ) -> ClientResult<ProposeBurstsResponse> {
        if request.k == 0 || request.k > MAX_PROPOSE_BURSTS {
            return Err(ClientError::new(
                ClientErrorKind::InvalidRequest,
                format!("k must be in 1..={MAX_PROPOSE_BURSTS}, got {}", request.k),
            ));
        }

        let Some(experiment) = self.loaded_experiments.get(&request.experiment_id) else {
            return Err(ClientError::new(
                ClientErrorKind::InvalidRequest,
                format!("unknown experiment_id '{}'", request.experiment_id),
            ));
        };
        if experiment.model != request.model {
            return Err(ClientError::new(
                ClientErrorKind::InvalidRequest,
                "request model does not match loaded experiment config",
            ));
        }

        let requested_mix = experiment
            .mix
            .with_overrides(&request.config_overrides_yaml)?;
        let base_fingerprint = self.config_fingerprint(experiment, requested_mix);
        let request_identity = propose_request_identity(&request, base_fingerprint);
        let fault_decision = self.fault_injector.decide(
            FaultRequest::new(FaultTarget::Synth, "propose_bursts", &request_identity),
            request.k,
        );
        self.last_fault.set(Some(fault_decision));
        if let Some(error) = fault_decision.client_error() {
            return Err(error);
        }

        let final_fingerprint = fault_decision.apply_synth_fingerprint(base_fingerprint);
        let (effective_mix, degraded, weighted_fallback_from) =
            self.effective_mix(requested_mix, &request);
        let response_len = request.k as usize;
        let mut bursts = Vec::with_capacity(response_len);
        for slot in 0..response_len as u32 {
            let generator = effective_mix.select_generator(request.seed, slot);
            let fallback_from =
                (generator == GeneratorKind::WeightedRandom).then_some(weighted_fallback_from);
            let fallback_from = fallback_from.flatten();
            let burst = match request.model {
                ModelKind::Pad => self.propose_pad_burst(
                    &request,
                    slot,
                    generator,
                    fallback_from,
                    final_fingerprint,
                ),
                ModelKind::EventGrammar => self.propose_event_burst(
                    &request,
                    slot,
                    generator,
                    fallback_from,
                    final_fingerprint,
                ),
            };
            bursts.push(burst);
        }

        Ok(ProposeBurstsResponse {
            bursts,
            config_fingerprint: final_fingerprint,
            synth_version: self.synth_version.clone(),
            seed: request.seed,
            degraded,
        })
    }

    fn mine_macros(&mut self, request: MineMacrosRequest) -> ClientResult<MineMacrosResponse> {
        let pack_id = format!("mined-{}", stable_hex_id(request.experiment_id.as_bytes()));
        let paths_used = request.paths.len() as u32;
        let tokens_scanned: u64 = request
            .paths
            .iter()
            .flat_map(|path| path.expansions.iter())
            .map(|scored| burst_token_count(&scored.burst.burst))
            .sum();
        let score_value = request
            .paths
            .iter()
            .map(|path| path.terminal_score.get())
            .fold(0.0, f64::max);
        let score = Score::new(score_value).expect("max finite scores stay finite");

        Ok(MineMacrosResponse {
            macro_pack_yaml: format!(
                "version: 1\nkind: macro_pack\nname: {pack_id}\nmodel: pad\nmacros:\n  - name: mined-{paths_used}\n"
            )
            .into_bytes(),
            pack_id,
            stats: vec![MinedMacroStats {
                name: format!("mined-{paths_used}"),
                support: paths_used,
                paths: paths_used,
                lift: FiniteF64::new(1.0).expect("finite"),
                score,
                len_tokens: tokens_scanned.min(u64::from(u32::MAX)) as u32,
            }],
            paths_used,
            tokens_scanned,
        })
    }
}

impl FakeSynth {
    fn effective_mix(
        &self,
        requested_mix: GeneratorMix,
        request: &ProposeBurstsRequest,
    ) -> (GeneratorMix, Vec<DegradedGenerator>, Option<GeneratorKind>) {
        let mut mix = requested_mix;
        let mut degraded = Vec::new();
        let mut weighted_fallback_from = None;

        if mix.macro_weight > 0.0 && self.loaded_packs.is_empty() {
            degraded.push(DegradedGenerator {
                generator: GeneratorKind::Macro,
                reason: "no_macros_loaded".to_owned(),
            });
            mix.weighted_random += mix.macro_weight;
            mix.macro_weight = 0.0;
            if requested_mix.weighted_random == 0.0 {
                weighted_fallback_from = Some(GeneratorKind::Macro);
            }
        }

        if mix.mutation > 0.0
            && request.node_context.parent_burst.is_none()
            && request.node_context.sibling_bursts.is_empty()
        {
            degraded.push(DegradedGenerator {
                generator: GeneratorKind::Mutation,
                reason: "no_parent_burst".to_owned(),
            });
            mix.weighted_random += mix.mutation;
            mix.mutation = 0.0;
            if requested_mix.weighted_random == 0.0 && weighted_fallback_from.is_none() {
                weighted_fallback_from = Some(GeneratorKind::Mutation);
            }
        }

        if mix.policy > 0.0 {
            degraded.push(DegradedGenerator {
                generator: GeneratorKind::Policy,
                reason: "policy_endpoint_down".to_owned(),
            });
            mix.weighted_random += mix.policy;
            mix.policy = 0.0;
            if requested_mix.weighted_random == 0.0 && weighted_fallback_from.is_none() {
                weighted_fallback_from = Some(GeneratorKind::Policy);
            }
        }

        if mix.total() <= 0.0 {
            mix.weighted_random = 1.0;
        }

        (mix, degraded, weighted_fallback_from)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct FakeExperimentConfig {
    experiment_id: String,
    model: ModelKind,
    button_alphabet: String,
    mix: GeneratorMix,
}

#[derive(Clone, Debug, PartialEq)]
struct FakeMacroPack {
    pack_id: String,
    macros: Vec<FakeMacro>,
}

#[derive(Clone, Debug, PartialEq)]
struct FakeMacro {
    name: String,
    weight: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GeneratorMix {
    weighted_random: f64,
    macro_weight: f64,
    mutation: f64,
    policy: f64,
}

impl Default for GeneratorMix {
    fn default() -> Self {
        Self {
            weighted_random: 1.0,
            macro_weight: 0.0,
            mutation: 0.0,
            policy: 0.0,
        }
    }
}

impl GeneratorMix {
    fn with_config_document(self, bytes: &[u8]) -> ClientResult<Self> {
        let text = str::from_utf8(bytes).map_err(|_| {
            ClientError::new(
                ClientErrorKind::InvalidRequest,
                "experiment config document is not valid UTF-8",
            )
        })?;
        if !has_generator_mix_key(text) {
            return Ok(self.clamp_nonnegative());
        }

        self.with_overrides(bytes)
    }

    fn with_overrides(mut self, bytes: &[u8]) -> ClientResult<Self> {
        let text = str::from_utf8(bytes).map_err(|_| {
            ClientError::new(
                ClientErrorKind::InvalidRequest,
                "config_overrides_yaml is not valid UTF-8",
            )
        })?;
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed == "{}" {
            return Ok(self.clamp_nonnegative());
        }
        if !has_generator_mix_key(text) {
            return Err(ClientError::new(
                ClientErrorKind::InvalidRequest,
                "config_overrides_yaml must contain a generator_mix override",
            ));
        }

        for (key, value) in parse_generator_mix_entries(text)? {
            match key.as_str() {
                "weighted_random" => self.weighted_random = value,
                "macro" => self.macro_weight = value,
                "mutation" => self.mutation = value,
                "policy" => self.policy = value,
                _ => {
                    return Err(ClientError::new(
                        ClientErrorKind::InvalidRequest,
                        format!("unknown generator_mix key '{key}'"),
                    ));
                }
            }
        }
        Ok(self.clamp_nonnegative())
    }

    fn clamp_nonnegative(mut self) -> Self {
        self.weighted_random = self.weighted_random.max(0.0);
        self.macro_weight = self.macro_weight.max(0.0);
        self.mutation = self.mutation.max(0.0);
        self.policy = self.policy.max(0.0);
        self
    }

    fn total(self) -> f64 {
        self.weighted_random + self.macro_weight + self.mutation + self.policy
    }

    fn select_generator(self, seed: u64, slot: u32) -> GeneratorKind {
        let total = self.total();
        if total <= 0.0 {
            return GeneratorKind::WeightedRandom;
        }

        let mut rng = DeterministicRng::synth(seed, u64::from(slot));
        let mut cursor = rng.next_unit_f64() * total;
        for (generator, weight) in [
            (GeneratorKind::WeightedRandom, self.weighted_random),
            (GeneratorKind::Macro, self.macro_weight),
            (GeneratorKind::Mutation, self.mutation),
            (GeneratorKind::Policy, self.policy),
        ] {
            if weight <= 0.0 {
                continue;
            }
            if cursor < weight {
                return generator;
            }
            cursor -= weight;
        }
        GeneratorKind::WeightedRandom
    }

    fn update_hash(self, hasher: &mut blake3::Hasher) {
        hasher.update(&self.weighted_random.to_bits().to_le_bytes());
        hasher.update(&self.macro_weight.to_bits().to_le_bytes());
        hasher.update(&self.mutation.to_bits().to_le_bytes());
        hasher.update(&self.policy.to_bits().to_le_bytes());
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LoadedDocument {
    document_id: String,
    bytes: Vec<u8>,
    warnings: Vec<String>,
}

impl LoadedDocument {
    fn from_source(source: LoadMacroPackSource, fallback_prefix: &str) -> Self {
        match source {
            LoadMacroPackSource::DocumentYaml(bytes) => Self {
                document_id: format!("{fallback_prefix}-{}", stable_hex_id(&bytes)),
                bytes,
                warnings: Vec::new(),
            },
            LoadMacroPackSource::ArtifactRef(reference) => Self {
                document_id: reference
                    .rsplit('/')
                    .next()
                    .filter(|value| !value.is_empty())
                    .unwrap_or(reference.as_str())
                    .to_owned(),
                bytes: format!(
                    "version: 1\nkind: {fallback_prefix}\nexperiment_id: {reference}\nname: {reference}\n"
                )
                .into_bytes(),
                warnings: vec!["artifact_ref_loaded_as_synthetic_document".to_owned()],
            },
        }
    }

    fn text(&self) -> ClientResult<&str> {
        str::from_utf8(&self.bytes).map_err(|_| {
            ClientError::new(
                ClientErrorKind::InvalidRequest,
                "macro/config document is not valid UTF-8",
            )
        })
    }
}

fn target_frames(length_hint: FrameCount) -> u32 {
    let frames = length_hint.get();
    if frames == 0 {
        16
    } else {
        frames.clamp(1, 1_800)
    }
}

fn weighted_random_segments(rng: &mut DeterministicRng, target_frames: u32) -> Vec<PadSegment> {
    let segment_count = 1 + draw_index(rng, 3);
    let mut remaining = target_frames.max(1);
    let mut segments = Vec::with_capacity(segment_count as usize);

    for index in 0..segment_count {
        let is_last = index + 1 == segment_count;
        let hold_frames = if is_last {
            remaining
        } else {
            let max_take = remaining.saturating_sub(segment_count - index - 1).max(1);
            1 + draw_index(rng, max_take)
        };
        remaining = remaining.saturating_sub(hold_frames);
        segments.push(PadSegment {
            buttons: BUTTON_MASKS[draw_index(rng, BUTTON_MASKS.len() as u32) as usize],
            hold_frames: FrameCount::new(hold_frames),
        });
    }

    segments
}

fn macro_segments(
    selected_macro: &FakeMacro,
    direction: &str,
    target_frames: u32,
) -> Vec<PadSegment> {
    let direction_mask = match direction {
        "left" => 0b1_0000_0000,
        "right" => 0b10_0000_0000,
        _ => 0,
    };
    let action_mask =
        if stable_u64(selected_macro.name.as_bytes(), b"macro-action").is_multiple_of(2) {
            0b0000_0001
        } else {
            0b0000_0010
        };
    let first = target_frames.clamp(1, 24);
    let second = target_frames.saturating_sub(first);
    let mut segments = vec![PadSegment {
        buttons: direction_mask | action_mask,
        hold_frames: FrameCount::new(first),
    }];
    if second > 0 {
        segments.push(PadSegment {
            buttons: direction_mask,
            hold_frames: FrameCount::new(second),
        });
    }
    segments
}

fn mutate_parent_burst(
    request: &ProposeBurstsRequest,
    rng: &mut DeterministicRng,
    target_frames: u32,
) -> (BurstBody, MutationProvenance) {
    let base = request.node_context.parent_burst.as_ref().or_else(|| {
        request
            .node_context
            .sibling_bursts
            .first()
            .map(|scored| &scored.burst)
    });

    let Some(base) = base else {
        return (
            BurstBody::Pad(PadBurst {
                segments: weighted_random_segments(rng, target_frames),
                button_alphabet: BUTTON_ALPHABET.to_owned(),
            }),
            MutationProvenance {
                base_burst_id: Vec::new(),
                donor_burst_id: Vec::new(),
                base_was_sibling: false,
                ops: vec![MutationOp {
                    op: "fallback_weighted_random".to_owned(),
                    args: BTreeMap::new(),
                }],
                post_clamp: true,
            },
        );
    };

    let mut body = base.burst.body.clone();
    if let BurstBody::Pad(pad) = &mut body {
        if let Some(first) = pad.segments.first_mut() {
            let bit = draw_index(rng, 10);
            first.buttons ^= 1u32 << bit;
        }
    }

    (
        body,
        MutationProvenance {
            base_burst_id: base.burst.burst_id.as_bytes().to_vec(),
            donor_burst_id: Vec::new(),
            base_was_sibling: request.node_context.parent_burst.is_none(),
            ops: vec![MutationOp {
                op: "flip_button".to_owned(),
                args: BTreeMap::from([("bit".to_owned(), "deterministic".to_owned())]),
            }],
            post_clamp: true,
        },
    )
}

fn draw_index(rng: &mut DeterministicRng, upper: u32) -> u32 {
    ((rng.next_unit_f64() * f64::from(upper)).floor() as u32).min(upper.saturating_sub(1))
}

fn burst_id(seed: u64, slot: u32, fingerprint: ConfigFingerprint, body: &BurstBody) -> BurstId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"orch-fakes/synth-burst-id/v1");
    hasher.update(&seed.to_le_bytes());
    hasher.update(&slot.to_le_bytes());
    hasher.update(fingerprint.as_bytes());
    hasher.update(&postcard::to_allocvec(body).expect("burst body serializes"));
    BurstId::new(*hasher.finalize().as_bytes())
}

fn burst_token_count(burst: &Burst) -> u64 {
    match &burst.body {
        BurstBody::Pad(pad) => pad.segments.len() as u64,
        BurstBody::Event(event) => event.events.len() as u64,
    }
}

fn parse_model(text: &str) -> Option<ModelKind> {
    parse_scalar(text, "model").map(|value| {
        if value == "event_grammar" {
            ModelKind::EventGrammar
        } else {
            ModelKind::Pad
        }
    })
}

fn parse_scalar(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    text.lines().find_map(|line| {
        let trimmed = line.trim();
        let rest = trimmed.strip_prefix(&prefix)?;
        Some(clean_scalar(rest))
    })
}

fn parse_macro_names(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            trimmed.strip_prefix("- name:").map(clean_scalar)
        })
        .filter(|name| !matches!(name.as_str(), "macro_pack" | "pad" | "event_grammar"))
        .collect()
}

fn parse_generator_mix_entries(text: &str) -> ClientResult<Vec<(String, f64)>> {
    let mut entries = parse_generator_mix_block(text)?;
    entries.extend(parse_generator_mix_inline(text)?);
    Ok(entries)
}

fn parse_generator_mix_block(text: &str) -> ClientResult<Vec<(String, f64)>> {
    let mut in_mix = false;
    let mut entries = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("generator_mix:") {
            let rest = rest.trim();
            if !rest.is_empty() && !rest.starts_with('{') {
                return Err(ClientError::new(
                    ClientErrorKind::InvalidRequest,
                    "malformed generator_mix override",
                ));
            }
            in_mix = rest.is_empty();
            continue;
        }
        if !in_mix {
            continue;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') {
            break;
        }
        entries.push(parse_key_value_number(trimmed)?);
    }
    Ok(entries)
}

fn parse_generator_mix_inline(text: &str) -> ClientResult<Vec<(String, f64)>> {
    for line in text.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("generator_mix:") else {
            continue;
        };
        let rest = rest.trim();
        let Some(inner) = rest.strip_prefix('{') else {
            return Ok(Vec::new());
        };
        let Some(close) = inner.find('}') else {
            return Err(ClientError::new(
                ClientErrorKind::InvalidRequest,
                "malformed inline generator_mix override",
            ));
        };
        return inner[..close]
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(parse_key_value_number)
            .collect();
    }

    Ok(Vec::new())
}

fn parse_key_value_number(text: &str) -> ClientResult<(String, f64)> {
    let (key, value) = text.split_once(':').ok_or_else(|| {
        ClientError::new(
            ClientErrorKind::InvalidRequest,
            format!("malformed generator_mix entry '{text}'"),
        )
    })?;
    let key = clean_scalar(key);
    let value = clean_scalar(value);
    if key.is_empty() {
        return Err(ClientError::new(
            ClientErrorKind::InvalidRequest,
            "empty generator_mix key",
        ));
    }
    let value = value.parse::<f64>().map_err(|_| {
        ClientError::new(
            ClientErrorKind::InvalidRequest,
            format!("invalid generator_mix value for '{key}'"),
        )
    })?;
    if !value.is_finite() {
        return Err(ClientError::new(
            ClientErrorKind::InvalidRequest,
            format!("non-finite generator_mix value for '{key}'"),
        ));
    }
    Ok((key, value))
}

fn clean_scalar(value: &str) -> String {
    value
        .trim()
        .trim_matches(',')
        .trim_matches('"')
        .trim_matches('\'')
        .to_owned()
}

fn has_generator_mix_key(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim().starts_with("generator_mix:"))
}

fn propose_request_identity(
    request: &ProposeBurstsRequest,
    fingerprint: ConfigFingerprint,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"orch-fakes/synth-request/v1");
    update_len_prefixed(&mut hasher, request.experiment_id.as_bytes());
    hasher.update(&request.k.to_le_bytes());
    hasher.update(&request.length_hint.get().to_le_bytes());
    hasher.update(&request.seed.to_le_bytes());
    update_len_prefixed(&mut hasher, model_tag(request.model).as_bytes());
    update_len_prefixed(&mut hasher, request.config_overrides_yaml.as_slice());
    hasher.update(fingerprint.as_bytes());
    hasher.update(&request.node_context.node_id.get().to_le_bytes());
    hasher.update(request.node_context.state_hash.as_bytes());
    hasher.update(&request.node_context.cell_key.get().to_le_bytes());
    *hasher.finalize().as_bytes()
}

fn stable_hex_id(bytes: &[u8]) -> String {
    let digest = blake3::hash(bytes);
    digest.as_bytes()[..8]
        .iter()
        .fold(String::new(), |mut out, byte| {
            out.push(hex_char(byte >> 4));
            out.push(hex_char(byte & 0x0f));
            out
        })
}

fn stable_u64(bytes: &[u8], domain: &[u8]) -> u64 {
    let mut hasher = blake3::Hasher::new();
    update_len_prefixed(&mut hasher, domain);
    update_len_prefixed(&mut hasher, bytes);
    let digest = hasher.finalize();
    u64::from_le_bytes(
        digest.as_bytes()[..8]
            .try_into()
            .expect("slice has 8 bytes"),
    )
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

fn model_tag(model: ModelKind) -> &'static str {
    match model {
        ModelKind::Pad => "pad",
        ModelKind::EventGrammar => "event_grammar",
    }
}

fn generator_tag(generator: GeneratorKind) -> &'static str {
    match generator {
        GeneratorKind::WeightedRandom => "weighted_random",
        GeneratorKind::Macro => "macro",
        GeneratorKind::Mutation => "mutation",
        GeneratorKind::Policy => "policy",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_clients::input_synth::{ScoredBurst, CONFIG_FINGERPRINT_LEN};
    use orch_core::types::{CellKey, NodeId, Novelty, SnapshotRef, Stage, StateHash};

    #[test]
    fn synth_same_request_seed_produces_identical_bursts() {
        let mut first = configured_synth(true);
        let mut second = configured_synth(true);
        let request = sample_request(123, b"");

        assert_eq!(
            first
                .propose_bursts(request.clone())
                .expect("first response"),
            second.propose_bursts(request).expect("second response")
        );
    }

    #[test]
    fn synth_different_seed_changes_bursts() {
        let mut synth = configured_synth(true);

        let a = synth
            .propose_bursts(sample_request(123, b""))
            .expect("seed a response");
        let b = synth
            .propose_bursts(sample_request(124, b""))
            .expect("seed b response");

        assert_eq!(a.bursts.len(), 4);
        assert_eq!(b.bursts.len(), 4);
        assert_ne!(a.bursts, b.bursts);
        assert_eq!(a.seed, 123);
        assert_eq!(b.seed, 124);
    }

    #[test]
    fn synth_macro_override_affects_distribution_deterministically() {
        let mut first = configured_synth(true);
        let mut second = configured_synth(true);
        let override_yaml = b"generator_mix:\n  weighted_random: 0\n  macro: 1\n";

        let macro_response = first
            .propose_bursts(sample_request(99, override_yaml))
            .expect("macro response");
        let repeat = second
            .propose_bursts(sample_request(99, override_yaml))
            .expect("repeat response");
        let weighted = first
            .propose_bursts(sample_request(
                99,
                b"generator_mix:\n  weighted_random: 1\n  macro: 0\n",
            ))
            .expect("weighted response");

        assert_eq!(macro_response, repeat);
        assert!(macro_response
            .bursts
            .iter()
            .all(|burst| burst.provenance.generator == GeneratorKind::Macro));
        assert_ne!(macro_response.bursts, weighted.bursts);
        assert_ne!(
            macro_response.config_fingerprint,
            weighted.config_fingerprint
        );
    }

    #[test]
    fn synth_health_reports_loaded_packs_and_experiments() {
        let synth = configured_synth(true);

        let health = synth.health(HealthRequest).expect("health");

        assert_eq!(health.status, HealthStatus::Serving);
        assert_eq!(health.synth_version, FAKE_SYNTH_VERSION);
        assert_eq!(health.loaded_experiments, ["exp-a"]);
        assert_eq!(health.loaded_packs, ["console16-movement-core"]);
        assert!(!health.policy_endpoint_up);
        assert!(health.policy_deterministic);
    }

    #[test]
    fn synth_degraded_macro_less_mode_is_explicit() {
        let mut synth = configured_synth(false);
        let override_yaml = b"generator_mix:\n  weighted_random: 0\n  macro: 1\n";

        let response = synth
            .propose_bursts(sample_request(55, override_yaml))
            .expect("macro-less response");

        assert_eq!(response.degraded.len(), 1);
        assert_eq!(response.degraded[0].generator, GeneratorKind::Macro);
        assert_eq!(response.degraded[0].reason, "no_macros_loaded");
        assert!(response.bursts.iter().all(|burst| {
            burst.provenance.generator == GeneratorKind::WeightedRandom
                && burst.provenance.fallback_from == Some(GeneratorKind::Macro)
        }));
    }

    #[test]
    fn synth_partial_response_fault_does_not_truncate_exact_k_contract() {
        let mut synth = configured_synth_with_fault(
            true,
            FaultPlan::disabled(0xfeed).with_partial_response(
                crate::fault::PartialResponseFault::new(crate::fault::FaultRate::always(), 0),
            ),
        );

        let response = synth
            .propose_bursts(sample_request(66, b""))
            .expect("partial response fault should not alter synth shape");

        assert_eq!(response.bursts.len(), 4);
        assert_eq!(
            response
                .bursts
                .iter()
                .map(|burst| burst.provenance.slot)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn synth_malformed_overrides_are_invalid_request() {
        for overrides in [
            b"generator_mix:\n  macro: nope\n".as_slice(),
            b"generator_mix:\n  surprise: 1\n",
            b"generator_mix: { macro: NaN }\n",
            b"[",
            b"weighted_random: {",
            b"not_generator_mix: 1\n",
            &[0xff],
        ] {
            let mut synth = configured_synth(true);
            let error = synth
                .propose_bursts(sample_request(88, overrides))
                .expect_err("malformed overrides should be rejected");

            assert_eq!(error.kind(), ClientErrorKind::InvalidRequest);
        }
    }

    #[test]
    fn synth_fingerprint_flip_fault_is_observable() {
        let mut clean = configured_synth(true);
        let mut faulty = configured_synth_with_fault(
            true,
            FaultPlan::disabled(0x5eed)
                .with_synth_fingerprint_flip(crate::fault::FaultRate::always()),
        );
        let request = sample_request(77, b"");

        let clean_response = clean
            .propose_bursts(request.clone())
            .expect("clean response");
        let faulty_response = faulty.propose_bursts(request).expect("faulty response");

        assert_ne!(
            clean_response.config_fingerprint,
            faulty_response.config_fingerprint
        );
        assert_eq!(
            faulty_response.bursts[0].provenance.config_fingerprint,
            faulty_response.config_fingerprint
        );
        assert_eq!(
            clean_response.config_fingerprint.as_bytes().len(),
            CONFIG_FINGERPRINT_LEN
        );
    }

    fn configured_synth(load_macros: bool) -> FakeSynth {
        configured_synth_with_fault(load_macros, FaultPlan::disabled(0))
    }

    fn configured_synth_with_fault(load_macros: bool, fault_plan: FaultPlan) -> FakeSynth {
        let mut synth = FakeSynth::with_fault_plan(fault_plan);
        InputSynthClient::load_macro_pack(
            &mut synth,
            LoadMacroPackRequest {
                source: LoadMacroPackSource::DocumentYaml(
                    b"version: 1\nkind: experiment_config\nexperiment_id: exp-a\nmodel: pad\nbutton_alphabet: console16-12btn-v1\ngenerator_mix:\n  weighted_random: 1\n  macro: 0\n"
                        .to_vec(),
                ),
                kind: DocumentKind::ExperimentConfig,
            },
        )
        .expect("load experiment");
        if load_macros {
            let response = InputSynthClient::load_macro_pack(
                &mut synth,
                LoadMacroPackRequest {
                    source: LoadMacroPackSource::DocumentYaml(
                        b"version: 1\nkind: macro_pack\nname: console16-movement-core\nmodel: pad\nmacros:\n  - name: dash-right\n  - name: jump-arc\n"
                            .to_vec(),
                    ),
                    kind: DocumentKind::MacroPack,
                },
            )
            .expect("load macro pack");
            assert_eq!(response.items_loaded, 2);
        }
        synth
    }

    fn sample_request(seed: u64, overrides: &[u8]) -> ProposeBurstsRequest {
        ProposeBurstsRequest {
            experiment_id: "exp-a".to_owned(),
            node_context: sample_context(),
            k: 4,
            length_hint: FrameCount::new(32),
            seed,
            model: ModelKind::Pad,
            config_overrides_yaml: overrides.to_vec(),
        }
    }

    fn sample_context() -> orch_clients::input_synth::NodeContext {
        orch_clients::input_synth::NodeContext {
            node_id: NodeId::new(7),
            parent_node_id: Some(NodeId::ROOT),
            snapshot_ref: SnapshotRef::new([1; 32]),
            state_hash: StateHash::new([2; 32]),
            cell_key: CellKey::new(42),
            stage: Stage::new(3),
            depth: 4,
            frame_counter: FrameCount::new(900),
            node_score: Score::new(12.0).expect("finite score"),
            novelty: Novelty::new(0.5).expect("finite novelty"),
            ram_features: BTreeMap::from([(
                "boss_hp".to_owned(),
                FiniteF64::new(80.0).expect("finite"),
            )]),
            frame_embedding: vec![FiniteF64::new(0.125).expect("finite")],
            recent_inputs: None,
            parent_burst: None,
            sibling_bursts: Vec::<ScoredBurst>::new(),
        }
    }
}
