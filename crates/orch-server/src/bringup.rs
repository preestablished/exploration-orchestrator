//! Experiment bring-up (fresh or resume, ARCHITECTURE.md §3): compile the
//! feature map once for its two consumers, load it plus the scoring program
//! on the scorer with cross-checks, and bring the synthesizer up through
//! the transport-free `SynthBringup` composition.

use orch_clients::{
    hypervisor::{CaptureSpec, ExtractRange as HvExtractRange, MachineConfig},
    input_synth::{InputSynthClient, LoadMacroPackSource},
    scorer::{
        ArtifactSource, CompiledLayout as ScorerLayout, ExtractRange as ScorerExtractRange,
        LoadFeatureMapRequest, LoadScoringProgramRequest,
    },
    ClientError, ClientErrorKind, ClientResult,
};
use orch_core::{
    compile::{compile_feature_map, CompiledFeatureMap, FeatureMap, RegionLayouts},
    types::{ExperimentConfig, GuestInstructions},
};
use orch_driver::{
    input_synth::{SynthBringup, SynthBringupReport},
    node_attrs::{FpsRational, PadLayoutAttrs},
};
use orch_sched::ports::{AsyncScorer, SyncAdapter};

/// Workload-image manifest surrogate: on fakes there is no artifact
/// registry, so the caller supplies what the manifest would (MachineConfig,
/// framebuffer/fps/pad-layout attrs, bootstrap cap).
#[derive(Clone, Debug)]
pub struct WorkloadSpec {
    pub machine_config: MachineConfig,
    pub bootstrap_icount_cap: Option<GuestInstructions>,
    pub fps: Option<FpsRational>,
    pub pad_layout: Option<PadLayoutAttrs>,
}

/// Resolved artifact contents for one experiment (what `feature_map_ref`,
/// `scoring_program_ref`, `synth_config_ref`, and `macro_pack_refs` point
/// at, plus the workload manifest surrogate).
#[derive(Clone, Debug)]
pub struct ExperimentSources {
    pub feature_map: FeatureMap,
    pub region_layouts: RegionLayouts,
    pub synth_config_yaml: Vec<u8>,
    pub macro_pack_yamls: Vec<Vec<u8>>,
    pub workload: WorkloadSpec,
}

/// Everything bring-up establishes and the checkpoint pins.
#[derive(Clone, Debug)]
pub struct BringupOutcome {
    pub compiled: CompiledFeatureMap,
    pub capture: CaptureSpec,
    pub feature_map_hash: [u8; 32],
    pub scoring_program_hash: [u8; 32],
    pub stage_names: Vec<String>,
    pub synth: SynthBringup,
    pub synth_report: SynthBringupReport,
}

/// Startup-time sync access to the synthesizer so `SynthBringup::run` (the
/// trait-generic composition) is reused verbatim. Real async transports
/// replace this seam at M6 (disclosed constraint: the tonic adapter's
/// internal block_on Runtime must never run inside the async server).
pub trait SynthBringupPort {
    fn run_bringup(&self, bringup: &SynthBringup) -> ClientResult<SynthBringupReport>;
}

impl<T> SynthBringupPort for SyncAdapter<T>
where
    T: InputSynthClient + Send + 'static,
{
    fn run_bringup(&self, bringup: &SynthBringup) -> ClientResult<SynthBringupReport> {
        self.with_service_sync(|synth| bringup.run(synth))?
    }
}

fn scorer_layout(compiled: &CompiledFeatureMap) -> ScorerLayout {
    ScorerLayout {
        ranges: compiled
            .layout
            .ranges
            .iter()
            .map(|range| ScorerExtractRange {
                region: range.region.clone(),
                layout_version: range.layout_version,
                offset: range.offset,
                len: range.len,
            })
            .collect(),
    }
}

fn hypervisor_capture(compiled: &CompiledFeatureMap) -> CaptureSpec {
    CaptureSpec {
        ranges: compiled
            .layout
            .ranges
            .iter()
            .map(|range| HvExtractRange {
                region: range.region.clone(),
                layout_version: range.layout_version,
                offset: range.offset,
                len: range.len,
            })
            .collect(),
        framebuffer: false,
    }
}

/// Compiles the feature map (optionally the L4-coarsened document supplied
/// by the ladder) and brings the scorer + synthesizer up. `rebin` is set on
/// L4 re-bins and on resume at a re-binned version.
pub async fn bring_up<S, Sy>(
    config: &ExperimentConfig,
    experiment_id: &str,
    sources: &ExperimentSources,
    feature_map: &FeatureMap,
    rebin: bool,
    scorer: &S,
    synth: &Sy,
) -> ClientResult<BringupOutcome>
where
    S: AsyncScorer,
    Sy: SynthBringupPort,
{
    let compiled = compile_feature_map(feature_map, &sources.region_layouts).map_err(|error| {
        ClientError::new(
            ClientErrorKind::InvalidRequest,
            format!("feature map compilation failed: {error}"),
        )
    })?;

    let map_response = scorer
        .load_feature_map(LoadFeatureMapRequest {
            experiment_id: experiment_id.to_owned(),
            source: ArtifactSource::ArtifactRef(config.feature_map_ref.clone()),
            layout: scorer_layout(&compiled),
            frame: None,
            rebin,
        })
        .await?;
    if map_response.feature_bytes_len != compiled.total_len {
        return Err(ClientError::new(
            ClientErrorKind::FailedPrecondition,
            format!(
                "scorer feature_bytes_len {} != compiled total {} (one compilation, two consumers)",
                map_response.feature_bytes_len, compiled.total_len
            ),
        ));
    }

    let program_response = scorer
        .load_scoring_program(LoadScoringProgramRequest {
            experiment_id: experiment_id.to_owned(),
            source: ArtifactSource::ArtifactRef(config.scoring_program_ref.clone()),
        })
        .await?;

    let bringup = SynthBringup::from_sources(
        experiment_id.to_owned(),
        LoadMacroPackSource::DocumentYaml(sources.synth_config_yaml.clone()),
        sources
            .macro_pack_yamls
            .iter()
            .cloned()
            .map(LoadMacroPackSource::DocumentYaml)
            .collect(),
    )?;
    let synth_report = synth.run_bringup(&bringup)?;

    Ok(BringupOutcome {
        capture: hypervisor_capture(&compiled),
        feature_map_hash: map_response.feature_map_hash.into_bytes(),
        scoring_program_hash: program_response.program_hash.into_bytes(),
        stage_names: program_response.stage_names,
        compiled,
        synth: bringup,
        synth_report,
    })
}
