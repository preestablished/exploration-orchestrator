//! Deterministic fault-injection controls for fake service tests.

use std::fmt;

use orch_clients::{
    input_synth::{ConfigFingerprint, CONFIG_FINGERPRINT_LEN},
    ClientError, ClientErrorKind,
};

/// Fault-injection target used to keep per-service streams independent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FaultTarget {
    #[default]
    Hypervisor,
    SnapshotStore,
    Scorer,
    Synth,
    Grid,
}

impl FaultTarget {
    const fn tag(self) -> &'static [u8] {
        match self {
            Self::Hypervisor => b"hypervisor",
            Self::SnapshotStore => b"snapshot_store",
            Self::Scorer => b"scorer",
            Self::Synth => b"synth",
            Self::Grid => b"grid",
        }
    }
}

/// Stable request identity for deterministic fault decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultRequest<'a> {
    pub target: FaultTarget,
    pub operation: &'a str,
    pub request_identity: &'a [u8],
}

impl<'a> FaultRequest<'a> {
    #[must_use]
    pub const fn new(target: FaultTarget, operation: &'a str, request_identity: &'a [u8]) -> Self {
        Self {
            target,
            operation,
            request_identity,
        }
    }
}

/// One-million denominator rate knob for deterministic fault branches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FaultRate {
    per_million: u32,
}

impl FaultRate {
    pub const DENOMINATOR: u32 = 1_000_000;

    #[must_use]
    pub const fn never() -> Self {
        Self { per_million: 0 }
    }

    #[must_use]
    pub const fn always() -> Self {
        Self {
            per_million: Self::DENOMINATOR,
        }
    }

    pub fn per_million(per_million: u32) -> Result<Self, FaultConfigError> {
        if per_million > Self::DENOMINATOR {
            return Err(FaultConfigError::RateOutOfRange { per_million });
        }

        Ok(Self { per_million })
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.per_million
    }

    fn fires(self, draw: u64) -> bool {
        if self.per_million == 0 {
            return false;
        }
        if self.per_million == Self::DENOMINATOR {
            return true;
        }

        draw % u64::from(Self::DENOMINATOR) < u64::from(self.per_million)
    }
}

/// Invalid fault-plan configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultConfigError {
    RateOutOfRange { per_million: u32 },
}

impl fmt::Display for FaultConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RateOutOfRange { per_million } => write!(
                formatter,
                "fault rate {per_million} is greater than {} per million",
                FaultRate::DENOMINATOR
            ),
        }
    }
}

impl std::error::Error for FaultConfigError {}

/// Deterministic latency in fake service ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LatencyFault {
    pub base_ticks: u32,
    pub jitter_ticks: u32,
}

impl LatencyFault {
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            base_ticks: 0,
            jitter_ticks: 0,
        }
    }

    #[must_use]
    pub const fn new(base_ticks: u32, jitter_ticks: u32) -> Self {
        Self {
            base_ticks,
            jitter_ticks,
        }
    }

    fn decide(self, draw: u64) -> u32 {
        if self.jitter_ticks == 0 {
            return self.base_ticks;
        }

        self.base_ticks
            .saturating_add((draw % (u64::from(self.jitter_ticks) + 1)) as u32)
    }
}

/// Deterministic partial-response knob for collection-like fake responses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PartialResponseFault {
    pub rate: FaultRate,
    pub min_keep_items: u32,
}

impl PartialResponseFault {
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            rate: FaultRate::never(),
            min_keep_items: 0,
        }
    }

    #[must_use]
    pub const fn new(rate: FaultRate, min_keep_items: u32) -> Self {
        Self {
            rate,
            min_keep_items,
        }
    }

    fn decide(self, total_items: u32, fires_draw: u64, len_draw: u64) -> Option<PartialResponse> {
        if total_items == 0 || total_items <= self.min_keep_items || !self.rate.fires(fires_draw) {
            return None;
        }

        let min_keep = self.min_keep_items;
        let span = total_items - min_keep;
        let keep_items = min_keep + (len_draw % u64::from(span)) as u32;

        (keep_items < total_items).then_some(PartialResponse { keep_items })
    }
}

/// A deterministic partial-response decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PartialResponse {
    pub keep_items: u32,
}

impl PartialResponse {
    #[must_use]
    pub fn truncate_len(self, len: usize) -> usize {
        len.min(self.keep_items as usize)
    }
}

/// Terminal fault state for a fake request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FaultTerminal {
    #[default]
    None,
    Error(ClientErrorKind),
    Timeout,
}

/// Deterministic synthesizer fingerprint bit flip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SynthFingerprintFlip {
    pub byte_index: u8,
    pub bit_mask: u8,
}

impl SynthFingerprintFlip {
    #[must_use]
    pub fn apply(self, fingerprint: ConfigFingerprint) -> ConfigFingerprint {
        let mut bytes = fingerprint.into_bytes();
        bytes[usize::from(self.byte_index) % CONFIG_FINGERPRINT_LEN] ^= self.bit_mask;
        ConfigFingerprint::new(bytes)
    }
}

/// Full deterministic fault decision for one fake request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FaultDecision {
    pub latency_ticks: u32,
    pub terminal: FaultTerminal,
    pub partial: Option<PartialResponse>,
    pub synth_fingerprint_flip: Option<SynthFingerprintFlip>,
}

impl FaultDecision {
    #[must_use]
    pub fn client_error(self) -> Option<ClientError> {
        match self.terminal {
            FaultTerminal::None => None,
            FaultTerminal::Error(kind) => Some(ClientError::new(kind, "deterministic fake fault")),
            FaultTerminal::Timeout => Some(ClientError::new(
                ClientErrorKind::Unavailable,
                "deterministic fake timeout",
            )),
        }
    }

    #[must_use]
    pub fn truncate_len(self, len: usize) -> usize {
        self.partial
            .map_or(len, |partial| partial.truncate_len(len))
    }

    #[must_use]
    pub fn apply_synth_fingerprint(self, fingerprint: ConfigFingerprint) -> ConfigFingerprint {
        self.synth_fingerprint_flip
            .map_or(fingerprint, |flip| flip.apply(fingerprint))
    }
}

/// Seed-pure fault plan shared by fake service implementations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaultPlan {
    seed: u64,
    latency: LatencyFault,
    error_rate: FaultRate,
    error_kind: ClientErrorKind,
    timeout_rate: FaultRate,
    partial_response: PartialResponseFault,
    synth_fingerprint_flip_rate: FaultRate,
}

impl FaultPlan {
    #[must_use]
    pub const fn disabled(seed: u64) -> Self {
        Self {
            seed,
            latency: LatencyFault::disabled(),
            error_rate: FaultRate::never(),
            error_kind: ClientErrorKind::Unavailable,
            timeout_rate: FaultRate::never(),
            partial_response: PartialResponseFault::disabled(),
            synth_fingerprint_flip_rate: FaultRate::never(),
        }
    }

    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    #[must_use]
    pub const fn latency(&self) -> LatencyFault {
        self.latency
    }

    #[must_use]
    pub const fn error_rate(&self) -> FaultRate {
        self.error_rate
    }

    #[must_use]
    pub const fn timeout_rate(&self) -> FaultRate {
        self.timeout_rate
    }

    #[must_use]
    pub const fn partial_response(&self) -> PartialResponseFault {
        self.partial_response
    }

    #[must_use]
    pub const fn synth_fingerprint_flip_rate(&self) -> FaultRate {
        self.synth_fingerprint_flip_rate
    }

    #[must_use]
    pub const fn with_latency(mut self, latency: LatencyFault) -> Self {
        self.latency = latency;
        self
    }

    #[must_use]
    pub const fn with_error(mut self, rate: FaultRate, kind: ClientErrorKind) -> Self {
        self.error_rate = rate;
        self.error_kind = kind;
        self
    }

    #[must_use]
    pub const fn with_timeout(mut self, rate: FaultRate) -> Self {
        self.timeout_rate = rate;
        self
    }

    #[must_use]
    pub const fn with_partial_response(mut self, partial_response: PartialResponseFault) -> Self {
        self.partial_response = partial_response;
        self
    }

    #[must_use]
    pub const fn with_synth_fingerprint_flip(mut self, rate: FaultRate) -> Self {
        self.synth_fingerprint_flip_rate = rate;
        self
    }
}

/// Deterministic fault injector used by fakes at request boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaultInjector {
    plan: FaultPlan,
}

impl FaultInjector {
    #[must_use]
    pub const fn new(plan: FaultPlan) -> Self {
        Self { plan }
    }

    #[must_use]
    pub const fn plan(&self) -> &FaultPlan {
        &self.plan
    }

    #[must_use]
    pub fn decide(&self, request: FaultRequest<'_>, response_items: u32) -> FaultDecision {
        let latency_ticks =
            self.plan
                .latency
                .decide(draw_u64(self.plan.seed, request, FaultChannel::Latency));

        let terminal = if self.plan.timeout_rate.fires(draw_u64(
            self.plan.seed,
            request,
            FaultChannel::Timeout,
        )) {
            FaultTerminal::Timeout
        } else if self
            .plan
            .error_rate
            .fires(draw_u64(self.plan.seed, request, FaultChannel::Error))
        {
            FaultTerminal::Error(self.plan.error_kind)
        } else {
            FaultTerminal::None
        };

        let has_response = matches!(terminal, FaultTerminal::None);
        let partial = has_response
            .then(|| {
                self.plan.partial_response.decide(
                    response_items,
                    draw_u64(self.plan.seed, request, FaultChannel::PartialResponse),
                    draw_u64(self.plan.seed, request, FaultChannel::PartialResponseLen),
                )
            })
            .flatten();

        let synth_fingerprint_flip = (has_response
            && request.target == FaultTarget::Synth
            && self.plan.synth_fingerprint_flip_rate.fires(draw_u64(
                self.plan.seed,
                request,
                FaultChannel::SynthFingerprintFlip,
            )))
        .then(|| {
            let draw = draw_u64(
                self.plan.seed,
                request,
                FaultChannel::SynthFingerprintFlipBit,
            );
            SynthFingerprintFlip {
                byte_index: (draw % CONFIG_FINGERPRINT_LEN as u64) as u8,
                bit_mask: 1u8 << ((draw >> 8) % 8) as u32,
            }
        });

        FaultDecision {
            latency_ticks,
            terminal,
            partial,
            synth_fingerprint_flip,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum FaultChannel {
    Latency,
    Error,
    Timeout,
    PartialResponse,
    PartialResponseLen,
    SynthFingerprintFlip,
    SynthFingerprintFlipBit,
}

impl FaultChannel {
    const fn tag(self) -> &'static [u8] {
        match self {
            Self::Latency => b"latency",
            Self::Error => b"error",
            Self::Timeout => b"timeout",
            Self::PartialResponse => b"partial_response",
            Self::PartialResponseLen => b"partial_response_len",
            Self::SynthFingerprintFlip => b"synth_fingerprint_flip",
            Self::SynthFingerprintFlipBit => b"synth_fingerprint_flip_bit",
        }
    }
}

fn draw_u64(seed: u64, request: FaultRequest<'_>, channel: FaultChannel) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"orch-fakes/fault/v1");
    hasher.update(&seed.to_le_bytes());
    hasher.update(channel.tag());
    hasher.update(request.target.tag());
    update_len_prefixed(&mut hasher, request.operation.as_bytes());
    update_len_prefixed(&mut hasher, request.request_identity);

    let bytes = hasher.finalize();
    u64::from_le_bytes(bytes.as_bytes()[..8].try_into().expect("slice has 8 bytes"))
}

fn update_len_prefixed(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    const FINGERPRINT: ConfigFingerprint = ConfigFingerprint::new([0xA5; 32]);
    const REQUEST_A: FaultRequest<'static> =
        FaultRequest::new(FaultTarget::Synth, "propose_bursts", b"node=7;batch=3");
    const REQUEST_B: FaultRequest<'static> =
        FaultRequest::new(FaultTarget::Synth, "propose_bursts", b"node=8;batch=3");

    #[test]
    fn fault_same_seed_and_request_gives_same_outcome() {
        let injector = FaultInjector::new(
            FaultPlan::disabled(0x5eed)
                .with_latency(LatencyFault::new(3, 19))
                .with_partial_response(PartialResponseFault::new(FaultRate::always(), 1))
                .with_synth_fingerprint_flip(FaultRate::always()),
        );

        let first = injector.decide(REQUEST_A, 16);
        let second = injector.decide(REQUEST_A, 16);

        assert_eq!(first, second);
        assert!((3..=22).contains(&first.latency_ticks));
        assert_eq!(first.terminal, FaultTerminal::None);
        assert!(first.partial.expect("partial should fire").keep_items < 16);
        assert_ne!(first.apply_synth_fingerprint(FINGERPRINT), FINGERPRINT);
    }

    #[test]
    fn fault_different_seed_or_request_varies_outcome() {
        let plan = |seed| {
            FaultPlan::disabled(seed)
                .with_latency(LatencyFault::new(0, 512))
                .with_partial_response(PartialResponseFault::new(FaultRate::always(), 0))
                .with_synth_fingerprint_flip(FaultRate::always())
        };

        let seed_a = FaultInjector::new(plan(11)).decide(REQUEST_A, 32);
        let seed_b = FaultInjector::new(plan(12)).decide(REQUEST_A, 32);
        let request_b = FaultInjector::new(plan(11)).decide(REQUEST_B, 32);

        assert_ne!(seed_a, seed_b);
        assert_ne!(seed_a, request_b);
    }

    #[test]
    fn fault_terminal_timeout_precedes_error_and_suppresses_response_faults() {
        let injector = FaultInjector::new(
            FaultPlan::disabled(9)
                .with_error(FaultRate::always(), ClientErrorKind::DataLoss)
                .with_timeout(FaultRate::always())
                .with_partial_response(PartialResponseFault::new(FaultRate::always(), 0))
                .with_synth_fingerprint_flip(FaultRate::always()),
        );

        let decision = injector.decide(REQUEST_A, 10);

        assert_eq!(decision.terminal, FaultTerminal::Timeout);
        assert_eq!(
            decision.client_error().expect("timeout error").kind(),
            ClientErrorKind::Unavailable
        );
        assert_eq!(decision.partial, None);
        assert_eq!(decision.synth_fingerprint_flip, None);
    }

    #[test]
    fn fault_error_rate_maps_to_configured_error_kind() {
        let injector = FaultInjector::new(
            FaultPlan::disabled(9).with_error(FaultRate::always(), ClientErrorKind::DataLoss),
        );

        let decision = injector.decide(REQUEST_A, 10);

        assert_eq!(
            decision.terminal,
            FaultTerminal::Error(ClientErrorKind::DataLoss)
        );
        assert_eq!(
            decision.client_error().expect("configured error").kind(),
            ClientErrorKind::DataLoss
        );
    }

    #[test]
    fn fault_partial_response_only_truncates_when_meaningful() {
        let injector = FaultInjector::new(
            FaultPlan::disabled(42)
                .with_partial_response(PartialResponseFault::new(FaultRate::always(), 4)),
        );

        let partial = injector.decide(REQUEST_A, 12);
        let too_small = injector.decide(REQUEST_A, 1);

        assert!(partial.truncate_len(12) < 12);
        assert_eq!(too_small.partial, None);
        assert_eq!(too_small.truncate_len(1), 1);
    }

    #[test]
    fn fault_disabled_knobs_leave_baseline_transcripts_unchanged() {
        for seed in [0, 1, u64::MAX] {
            for request in [REQUEST_A, REQUEST_B] {
                let decision = FaultInjector::new(FaultPlan::disabled(seed)).decide(request, 24);

                assert_eq!(decision, FaultDecision::default());
                assert_eq!(decision.truncate_len(24), 24);
                assert_eq!(decision.apply_synth_fingerprint(FINGERPRINT), FINGERPRINT);
                assert_eq!(decision.client_error(), None);
            }
        }
    }

    #[test]
    fn fault_rejects_out_of_range_rate_knobs() {
        assert_eq!(
            FaultRate::per_million(FaultRate::DENOMINATOR + 1),
            Err(FaultConfigError::RateOutOfRange {
                per_million: 1_000_001,
            })
        );
        assert_eq!(
            FaultRate::per_million(125_000).expect("valid rate").get(),
            125_000
        );
    }
}
