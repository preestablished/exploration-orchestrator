//! Deterministic fault-injection controls for fake service tests.

use std::{cell::RefCell, collections::BTreeMap, fmt};

use orch_clients::{
    input_synth::{ConfigFingerprint, CONFIG_FINGERPRINT_LEN},
    ClientError, ClientErrorKind,
};

/// Fault-injection target used to keep per-service streams independent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FaultTarget {
    #[default]
    Hypervisor,
    SnapshotStore,
    Scorer,
    Synth,
    Grid,
    Observatory,
}

impl FaultTarget {
    const fn tag(self) -> &'static [u8] {
        match self {
            Self::Hypervisor => b"hypervisor",
            Self::SnapshotStore => b"snapshot_store",
            Self::Scorer => b"scorer",
            Self::Synth => b"synth",
            Self::Grid => b"grid",
            Self::Observatory => b"observatory",
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
    InvalidSynthFingerprintFlip { byte_index: u8, bit_index: u8 },
}

impl fmt::Display for FaultConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RateOutOfRange { per_million } => write!(
                formatter,
                "fault rate {per_million} is greater than {} per million",
                FaultRate::DENOMINATOR
            ),
            Self::InvalidSynthFingerprintFlip {
                byte_index,
                bit_index,
            } => write!(
                formatter,
                "invalid synth fingerprint flip at byte {byte_index}, bit {bit_index}"
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
    byte_index: u8,
    bit_mask: u8,
}

impl SynthFingerprintFlip {
    pub fn new(byte_index: u8, bit_index: u8) -> Result<Self, FaultConfigError> {
        if usize::from(byte_index) >= CONFIG_FINGERPRINT_LEN || bit_index >= 8 {
            return Err(FaultConfigError::InvalidSynthFingerprintFlip {
                byte_index,
                bit_index,
            });
        }

        Ok(Self {
            byte_index,
            bit_mask: 1u8 << u32::from(bit_index),
        })
    }

    #[must_use]
    pub const fn byte_index(self) -> u8 {
        self.byte_index
    }

    #[must_use]
    pub const fn bit_mask(self) -> u8 {
        self.bit_mask
    }

    #[must_use]
    pub fn apply(self, fingerprint: ConfigFingerprint) -> ConfigFingerprint {
        let mut bytes = fingerprint.into_bytes();
        bytes[usize::from(self.byte_index)] ^= self.bit_mask;
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

    /// Adds error probability to the shared terminal-fault denominator.
    ///
    /// Timeout faults, when configured, own the first interval. Error faults
    /// use the next interval and are capped when `timeout_rate + error_rate`
    /// exceeds one million.
    #[must_use]
    pub const fn with_error(mut self, rate: FaultRate, kind: ClientErrorKind) -> Self {
        self.error_rate = rate;
        self.error_kind = kind;
        self
    }

    /// Adds timeout probability to the shared terminal-fault denominator.
    ///
    /// Timeout has first priority in that denominator. Error faults use the
    /// next interval and are capped when `timeout_rate + error_rate` exceeds
    /// one million.
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
///
/// Draws are salted with a per-`(target, operation)` attempt counter so a
/// retried request with identical bytes draws a fresh outcome, while a whole
/// run replayed against a fresh injector stays seed-deterministic. Ops whose
/// request identity is attempt-invariant (same `client_batch_id`, unit-struct
/// requests) would otherwise re-draw the identical terminal fault forever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaultInjector {
    plan: FaultPlan,
    attempts: RefCell<BTreeMap<(FaultTarget, String), u64>>,
}

impl FaultInjector {
    #[must_use]
    pub const fn new(plan: FaultPlan) -> Self {
        Self {
            plan,
            attempts: RefCell::new(BTreeMap::new()),
        }
    }

    #[must_use]
    pub const fn plan(&self) -> &FaultPlan {
        &self.plan
    }

    /// Decides the fault outcome for one request attempt and consumes the
    /// attempt counter for its `(target, operation)` stream.
    #[must_use]
    pub fn decide(&self, request: FaultRequest<'_>, response_items: u32) -> FaultDecision {
        let attempt = {
            let mut attempts = self.attempts.borrow_mut();
            let counter = attempts
                .entry((request.target, request.operation.to_owned()))
                .or_insert(0);
            let attempt = *counter;
            *counter += 1;
            attempt
        };
        self.decide_with_attempt(request, response_items, attempt)
    }

    /// Computes the decision the next [`Self::decide`] call for this
    /// `(target, operation)` stream would return, without consuming the
    /// attempt counter. Latency-probe seams use this to pre-charge virtual
    /// latency before making the (instant) sync call.
    #[must_use]
    pub fn peek(&self, request: FaultRequest<'_>, response_items: u32) -> FaultDecision {
        let attempt = self
            .attempts
            .borrow()
            .get(&(request.target, request.operation.to_owned()))
            .copied()
            .unwrap_or(0);
        self.decide_with_attempt(request, response_items, attempt)
    }

    fn decide_with_attempt(
        &self,
        request: FaultRequest<'_>,
        response_items: u32,
        attempt: u64,
    ) -> FaultDecision {
        let latency_ticks = self.plan.latency.decide(draw_u64(
            self.plan.seed,
            request,
            attempt,
            FaultChannel::Latency,
        ));

        let terminal = decide_terminal(
            self.plan.timeout_rate,
            self.plan.error_rate,
            self.plan.error_kind,
            draw_u64(self.plan.seed, request, attempt, FaultChannel::Terminal),
        );

        let has_response = matches!(terminal, FaultTerminal::None);
        let partial = has_response
            .then(|| {
                self.plan.partial_response.decide(
                    response_items,
                    draw_u64(
                        self.plan.seed,
                        request,
                        attempt,
                        FaultChannel::PartialResponse,
                    ),
                    draw_u64(
                        self.plan.seed,
                        request,
                        attempt,
                        FaultChannel::PartialResponseLen,
                    ),
                )
            })
            .flatten();

        let synth_fingerprint_flip = (has_response
            && request.target == FaultTarget::Synth
            && self.plan.synth_fingerprint_flip_rate.fires(draw_u64(
                self.plan.seed,
                request,
                attempt,
                FaultChannel::SynthFingerprintFlip,
            )))
        .then(|| {
            let draw = draw_u64(
                self.plan.seed,
                request,
                attempt,
                FaultChannel::SynthFingerprintFlipBit,
            );
            SynthFingerprintFlip::new(
                (draw % CONFIG_FINGERPRINT_LEN as u64) as u8,
                ((draw >> 8) % 8) as u8,
            )
            .expect("generated synth fingerprint flip is in range")
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
    Terminal,
    PartialResponse,
    PartialResponseLen,
    SynthFingerprintFlip,
    SynthFingerprintFlipBit,
}

impl FaultChannel {
    const fn tag(self) -> &'static [u8] {
        match self {
            Self::Latency => b"latency",
            Self::Terminal => b"terminal",
            Self::PartialResponse => b"partial_response",
            Self::PartialResponseLen => b"partial_response_len",
            Self::SynthFingerprintFlip => b"synth_fingerprint_flip",
            Self::SynthFingerprintFlipBit => b"synth_fingerprint_flip_bit",
        }
    }
}

fn decide_terminal(
    timeout_rate: FaultRate,
    error_rate: FaultRate,
    error_kind: ClientErrorKind,
    draw: u64,
) -> FaultTerminal {
    let terminal_draw = draw % u64::from(FaultRate::DENOMINATOR);
    let timeout_cutoff = u64::from(timeout_rate.get());
    let error_cutoff = timeout_cutoff
        .saturating_add(u64::from(error_rate.get()))
        .min(u64::from(FaultRate::DENOMINATOR));

    if terminal_draw < timeout_cutoff {
        FaultTerminal::Timeout
    } else if terminal_draw < error_cutoff {
        FaultTerminal::Error(error_kind)
    } else {
        FaultTerminal::None
    }
}

fn draw_u64(seed: u64, request: FaultRequest<'_>, attempt: u64, channel: FaultChannel) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"orch-fakes/fault/v2");
    hasher.update(&seed.to_le_bytes());
    hasher.update(&attempt.to_le_bytes());
    update_len_prefixed(&mut hasher, channel.tag());
    update_len_prefixed(&mut hasher, request.target.tag());
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
    fn fault_same_seed_and_call_sequence_gives_same_outcome() {
        let plan = FaultPlan::disabled(0x5eed)
            .with_latency(LatencyFault::new(3, 19))
            .with_partial_response(PartialResponseFault::new(FaultRate::always(), 1))
            .with_synth_fingerprint_flip(FaultRate::always());
        let replay_one = FaultInjector::new(plan.clone());
        let replay_two = FaultInjector::new(plan);

        let first_run: Vec<_> = (0..3).map(|_| replay_one.decide(REQUEST_A, 16)).collect();
        let second_run: Vec<_> = (0..3).map(|_| replay_two.decide(REQUEST_A, 16)).collect();

        assert_eq!(first_run, second_run);
        for decision in &first_run {
            assert!((3..=22).contains(&decision.latency_ticks));
            assert_eq!(decision.terminal, FaultTerminal::None);
            assert!(decision.partial.expect("partial should fire").keep_items < 16);
            assert_ne!(decision.apply_synth_fingerprint(FINGERPRINT), FINGERPRINT);
        }
    }

    #[test]
    fn fault_retries_draw_fresh_outcomes_for_identical_request_bytes() {
        let injector = FaultInjector::new(
            FaultPlan::disabled(0x5eed).with_latency(LatencyFault::new(0, u32::MAX)),
        );

        let draws: Vec<_> = (0..4)
            .map(|_| injector.decide(REQUEST_A, 16).latency_ticks)
            .collect();

        assert!(
            draws.windows(2).any(|pair| pair[0] != pair[1]),
            "attempt salt must vary retried draws: {draws:?}"
        );
    }

    #[test]
    fn fault_peek_previews_next_decision_without_consuming_attempt() {
        let injector = FaultInjector::new(
            FaultPlan::disabled(0x5eed).with_latency(LatencyFault::new(0, u32::MAX)),
        );

        let peeked = injector.peek(REQUEST_A, 16);
        assert_eq!(injector.peek(REQUEST_A, 16), peeked);
        assert_eq!(injector.decide(REQUEST_A, 16), peeked);

        let next_peek = injector.peek(REQUEST_A, 16);
        assert_eq!(injector.decide(REQUEST_A, 16), next_peek);
    }

    #[test]
    fn fault_attempt_streams_are_independent_per_target_and_operation() {
        let plan = FaultPlan::disabled(0x5eed).with_latency(LatencyFault::new(0, u32::MAX));
        let interleaved = FaultInjector::new(plan.clone());
        let solo = FaultInjector::new(plan);
        let other_op = FaultRequest::new(FaultTarget::Synth, "health", b"");
        let other_target = FaultRequest::new(FaultTarget::Scorer, "propose_bursts", b"");

        let first = interleaved.decide(REQUEST_A, 16);
        let _ = interleaved.decide(other_op, 16);
        let _ = interleaved.decide(other_target, 16);
        let second = interleaved.decide(REQUEST_A, 16);

        assert_eq!(solo.decide(REQUEST_A, 16), first);
        assert_eq!(solo.decide(REQUEST_A, 16), second);
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
    fn fault_partial_response_covers_boundary_lengths() {
        let min_zero = FaultInjector::new(
            FaultPlan::disabled(42)
                .with_partial_response(PartialResponseFault::new(FaultRate::always(), 0)),
        )
        .decide(REQUEST_A, 1);
        let min_plus_one = FaultInjector::new(
            FaultPlan::disabled(42)
                .with_partial_response(PartialResponseFault::new(FaultRate::always(), 4)),
        )
        .decide(REQUEST_A, 5);

        assert_eq!(
            min_zero.partial.expect("single item can truncate to empty"),
            PartialResponse { keep_items: 0 }
        );
        assert_eq!(min_zero.truncate_len(1), 0);
        assert_eq!(
            min_plus_one.partial.expect("min plus one can truncate"),
            PartialResponse { keep_items: 4 }
        );
        assert_eq!(min_plus_one.truncate_len(5), 4);
    }

    #[test]
    fn fault_terminal_rates_share_one_denominator() {
        let half = FaultRate::per_million(500_000).expect("valid rate");
        let quarter = FaultRate::per_million(250_000).expect("valid rate");

        assert_eq!(
            decide_terminal(half, half, ClientErrorKind::DataLoss, 499_999),
            FaultTerminal::Timeout
        );
        assert_eq!(
            decide_terminal(half, half, ClientErrorKind::DataLoss, 500_000),
            FaultTerminal::Error(ClientErrorKind::DataLoss)
        );
        assert_eq!(
            decide_terminal(half, half, ClientErrorKind::DataLoss, 999_999),
            FaultTerminal::Error(ClientErrorKind::DataLoss)
        );
        assert_eq!(
            decide_terminal(quarter, quarter, ClientErrorKind::DataLoss, 500_000),
            FaultTerminal::None
        );
    }

    #[test]
    fn fault_synth_fingerprint_flip_is_synth_only() {
        let injector = FaultInjector::new(
            FaultPlan::disabled(7).with_synth_fingerprint_flip(FaultRate::always()),
        );
        let non_synth = FaultRequest::new(FaultTarget::Scorer, "score_batch", b"batch=99");

        let synth_decision = injector.decide(REQUEST_A, 4);
        let scorer_decision = injector.decide(non_synth, 4);

        assert_ne!(
            synth_decision.apply_synth_fingerprint(FINGERPRINT),
            FINGERPRINT
        );
        assert_eq!(scorer_decision.synth_fingerprint_flip, None);
        assert_eq!(
            scorer_decision.apply_synth_fingerprint(FINGERPRINT),
            FINGERPRINT
        );
    }

    #[test]
    fn fault_synth_fingerprint_flip_rejects_noop_or_out_of_range_bits() {
        let flip = SynthFingerprintFlip::new(31, 7).expect("valid flip");

        assert_eq!(flip.byte_index(), 31);
        assert_eq!(flip.bit_mask(), 0b1000_0000);
        assert_ne!(flip.apply(FINGERPRINT), FINGERPRINT);
        assert_eq!(
            SynthFingerprintFlip::new(32, 0),
            Err(FaultConfigError::InvalidSynthFingerprintFlip {
                byte_index: 32,
                bit_index: 0,
            })
        );
        assert_eq!(
            SynthFingerprintFlip::new(0, 8),
            Err(FaultConfigError::InvalidSynthFingerprintFlip {
                byte_index: 0,
                bit_index: 8,
            })
        );
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
