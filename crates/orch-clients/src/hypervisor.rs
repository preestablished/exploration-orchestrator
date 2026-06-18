//! Hypervisor worker client boundary.
//!
//! Owner docs: `/home/infra-admin/.agents/projects/determinism/docs/determinism-hypervisor/API.md`
//! section 2 and
//! `/home/infra-admin/.agents/projects/determinism/docs/exploration-orchestrator/API.md`
//! section 2.
//!
//! This module mirrors the worker lease API used by the orchestrator: slot
//! lifecycle, input injection, bounded runs, snapshots, worker info, and slot
//! accounting. It intentionally does not expose an orchestrator job RPC or choose
//! a streaming/transport implementation.

use orch_core::types::{FrameCount, GuestInstructions, SnapshotRef, StateHash};
use serde::{Deserialize, Serialize};

pub use crate::snapshot_store::{InputLogId, INPUT_LOG_ID_LEN};

use crate::{ClientError, ClientErrorKind, ClientResult};

pub trait HypervisorWorkerClient {
    fn create_vm(&mut self, request: CreateVmRequest) -> ClientResult<CreateVmResponse>;

    fn restore_snapshot(
        &mut self,
        request: RestoreSnapshotRequest,
    ) -> ClientResult<RestoreSnapshotResponse>;

    fn fork(&mut self, request: ForkRequest) -> ClientResult<ForkResponse>;

    fn inject_inputs(&mut self, request: InjectInputsRequest)
        -> ClientResult<InjectInputsResponse>;

    fn run(&mut self, request: RunRequest) -> ClientResult<RunResponse>;

    fn take_snapshot(&mut self, request: TakeSnapshotRequest)
        -> ClientResult<TakeSnapshotResponse>;

    fn destroy_vm(&mut self, request: DestroyVmRequest) -> ClientResult<DestroyVmResponse>;

    fn list_slots(&self, request: ListSlotsRequest) -> ClientResult<ListSlotsResponse>;

    /// Returns the next transport-adapter batch from one slot-watch subscription.
    ///
    /// The owner API streams `SlotEvent`s. This trait keeps clients transport-free
    /// by representing a drained logical batch as one response; implementations
    /// must not synthesize watch results by repeatedly polling `ListSlots`.
    fn watch_slots(&self, request: WatchSlotsRequest) -> ClientResult<WatchSlotsResponse>;

    fn worker_info(&self, request: GetWorkerInfoRequest) -> ClientResult<GetWorkerInfoResponse>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateVmRequest {
    pub config: MachineConfig,
    pub entropy_seed: EntropySeed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateVmResponse {
    pub lease: Lease,
    pub icount: GuestInstructions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreSnapshotRequest {
    pub snapshot: SnapshotRef,
    pub entropy_seed: Option<EntropySeed>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreSnapshotResponse {
    pub lease: Lease,
    pub config: MachineConfig,
    pub state_hash: StateHash,
    pub frame_counter: FrameCount,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkRequest {
    pub parent: Lease,
    pub count: u32,
    pub entropy_seeds: Vec<EntropySeed>,
}

impl ForkRequest {
    pub fn new(parent: Lease, entropy_seeds: Vec<EntropySeed>) -> ClientResult<Self> {
        let count = u32::try_from(entropy_seeds.len()).map_err(|_| {
            ClientError::new(
                ClientErrorKind::InvalidRequest,
                "fork entropy seed count exceeds u32",
            )
        })?;
        let request = Self {
            parent,
            count,
            entropy_seeds,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> ClientResult<()> {
        if self.count == 0 {
            return Err(ClientError::new(
                ClientErrorKind::InvalidRequest,
                "fork count must be nonzero",
            ));
        }
        if self.entropy_seeds.len() != self.count as usize {
            return Err(ClientError::new(
                ClientErrorKind::InvalidRequest,
                "fork entropy seed count must match fork count",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkResponse {
    pub children: Vec<Lease>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestroyVmRequest {
    pub lease: Lease,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestroyVmResponse;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectInputsRequest {
    pub lease: Lease,
    pub events: Vec<ScheduledEvent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectInputsResponse {
    pub scheduled: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRequest {
    pub lease: Lease,
    pub until: RunUntil,
    pub hard_icount_cap: Option<GuestInstructions>,
    pub capture: Option<CaptureSpec>,
}

impl RunRequest {
    #[must_use]
    pub fn hard_icount_cap_wire_value(&self) -> u64 {
        self.hard_icount_cap.map_or(0, GuestInstructions::get)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunResponse {
    pub reason: StopReason,
    pub icount: GuestInstructions,
    pub vns: u64,
    pub state_hash: StateHash,
    pub frames_elapsed: u64,
    pub sdk_event: Option<GuestEvent>,
    pub feature_bytes: Option<Vec<u8>>,
    pub fb_lz4: Option<Vec<u8>>,
    pub fb_info: Option<FbInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakeSnapshotRequest {
    pub lease: Lease,
    pub seal_input_log: bool,
    pub capture: Option<CaptureSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakeSnapshotResponse {
    pub snapshot: SnapshotRef,
    pub input_log_id: Option<InputLogId>,
    pub icount: GuestInstructions,
    pub vns: u64,
    pub state_hash: StateHash,
    pub dirty_pages: u32,
    pub machine_config_hash: Digest32,
    pub determinism_class: DeterminismClass,
    pub feature_bytes: Option<Vec<u8>>,
    pub fb_lz4: Option<Vec<u8>>,
    pub fb_info: Option<FbInfo>,
    pub frame_counter: FrameCount,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetWorkerInfoRequest;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetWorkerInfoResponse {
    pub worker_id: String,
    pub slots_total: u32,
    pub slots_free: u32,
    pub class: DeterminismClass,
    pub version: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListSlotsRequest;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListSlotsResponse {
    pub slots: Vec<SlotInfo>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchSlotsRequest;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchSlotsResponse {
    pub events: Vec<SlotEvent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotEvent {
    pub slot: SlotInfo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub slot_id: SlotId,
    pub token: LeaseToken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SlotId(pub u64);

impl SlotId {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

pub const LEASE_TOKEN_LEN: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LeaseToken(pub [u8; LEASE_TOKEN_LEN]);

impl LeaseToken {
    #[must_use]
    pub const fn new(bytes: [u8; LEASE_TOKEN_LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; LEASE_TOKEN_LEN] {
        &self.0
    }

    #[must_use]
    pub const fn into_bytes(self) -> [u8; LEASE_TOKEN_LEN] {
        self.0
    }
}

pub const ENTROPY_SEED_LEN: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntropySeed(pub [u8; ENTROPY_SEED_LEN]);

impl EntropySeed {
    #[must_use]
    pub const fn new(bytes: [u8; ENTROPY_SEED_LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; ENTROPY_SEED_LEN] {
        &self.0
    }

    #[must_use]
    pub const fn into_bytes(self) -> [u8; ENTROPY_SEED_LEN] {
        self.0
    }
}

pub const DIGEST32_LEN: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Digest32(pub [u8; DIGEST32_LEN]);

impl Digest32 {
    #[must_use]
    pub const fn new(bytes: [u8; DIGEST32_LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; DIGEST32_LEN] {
        &self.0
    }

    #[must_use]
    pub const fn into_bytes(self) -> [u8; DIGEST32_LEN] {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineConfig {
    pub version: u32,
    pub mem_bytes: u64,
    pub vcpus: u32,
    pub clock_num: u32,
    pub clock_den: u32,
    pub base_image_hash: Digest32,
    pub boot: BootSpec,
    pub epoch_len: GuestInstructions,
    pub hash_epochs: HashEpochs,
    pub skid_margin: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BootSpec {
    Elf(ElfBoot),
    BzImage(BzImageBoot),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElfBoot {
    pub kernel_hash: Digest32,
    pub cmdline: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BzImageBoot {
    pub kernel_hash: Digest32,
    pub initramfs_hash: Digest32,
    pub cmdline: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HashEpochs {
    Unspecified,
    EpochsOn,
    FinalOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureSpec {
    pub ranges: Vec<ExtractRange>,
    pub framebuffer: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractRange {
    pub region: String,
    pub layout_version: u32,
    pub offset: u64,
    pub len: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FbInfo {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub frame_counter: FrameCount,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PixelFormat {
    Unspecified,
    Xrgb8888,
    Rgb565,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledEvent {
    pub at: ScheduleAt,
    pub event: InputEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduleAt {
    Icount(GuestInstructions),
    Vns(u64),
    Frame(FrameCount),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputEvent {
    PadSet(PadSet),
    DeviceEvent(DeviceEvent),
    NetRx(NetRx),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PadSet {
    pub port: u32,
    pub buttons: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceEvent {
    pub device_id: u32,
    pub event_type: u32,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetRx {
    pub frame: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunUntil {
    IcountBudget(GuestInstructions),
    VnsBudget(u64),
    FrameBudget(FrameCount),
    NextSdkEvent(NextSdkEvent),
    Goal(GoalCondition),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextSdkEvent {
    pub stream: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalCondition {
    pub all_of: Vec<MemPredicate>,
    pub poll_period: Option<GuestInstructions>,
}

impl GoalCondition {
    #[must_use]
    pub fn poll_period_wire_value(&self) -> u64 {
        self.poll_period.map_or(0, GuestInstructions::get)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemPredicate {
    pub gpa: u64,
    pub width: u32,
    pub mask: u64,
    pub op: PredicateOp,
    pub value: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PredicateOp {
    Unspecified,
    Eq,
    Ne,
    Ge,
    Le,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    Unspecified,
    BudgetReached,
    GoalSatisfied,
    NextSdkEvent,
    HardCap,
    Paused,
    GuestHalted,
    Faulted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestEvent {
    pub stream: u32,
    pub icount: GuestInstructions,
    pub vns: u64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeterminismClass {
    pub cpu_model: String,
    pub microcode: String,
    pub host_kernel: String,
    pub vmm_version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotInfo {
    pub slot_id: SlotId,
    pub state: SlotState,
    pub icount: GuestInstructions,
    pub base: Option<SnapshotRef>,
    pub live_children: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlotState {
    Unspecified,
    Empty,
    Paused,
    Running,
    Frozen,
    Faulted,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNAPSHOT: SnapshotRef = SnapshotRef::new([0xA5; 32]);
    const STATE: StateHash = StateHash::new([0x5A; 32]);
    const MACHINE_HASH: Digest32 = Digest32::new([0x33; DIGEST32_LEN]);
    const LOG_ID: InputLogId = InputLogId::new([0x44; INPUT_LOG_ID_LEN]);

    #[test]
    fn hypervisor_lease_token_and_slot_identity_are_fixed_width() {
        let lease = sample_lease(7, 0x11);

        assert_eq!(lease.slot_id, SlotId::new(7));
        assert_eq!(lease.slot_id.get(), 7);
        assert_eq!(lease.token.as_bytes(), &[0x11; LEASE_TOKEN_LEN]);
        assert_eq!(lease.token.into_bytes().len(), LEASE_TOKEN_LEN);
    }

    #[test]
    fn hypervisor_capture_spec_carries_ranges_and_framebuffer_request() {
        let capture = sample_capture();

        assert_eq!(capture.ranges.len(), 1);
        assert_eq!(capture.ranges[0].region, "wram");
        assert_eq!(capture.ranges[0].layout_version, 2);
        assert_eq!(capture.ranges[0].offset, 0x1000);
        assert_eq!(capture.ranges[0].len, 64);
        assert!(capture.framebuffer);
    }

    #[test]
    fn hypervisor_lifecycle_requests_carry_config_entropy_and_leases() {
        let create = CreateVmRequest {
            config: sample_config(),
            entropy_seed: EntropySeed::new([0x77; ENTROPY_SEED_LEN]),
        };
        let restore = RestoreSnapshotRequest {
            snapshot: SNAPSHOT,
            entropy_seed: None,
        };
        let fork = ForkRequest::new(
            sample_lease(1, 0x10),
            vec![
                EntropySeed::new([0x20; ENTROPY_SEED_LEN]),
                EntropySeed::new([0x21; ENTROPY_SEED_LEN]),
            ],
        )
        .expect("valid fork request");

        assert_eq!(create.config.vcpus, 1);
        assert_eq!(create.config.epoch_len, GuestInstructions::new(50_000_000));
        assert_eq!(create.entropy_seed.as_bytes(), &[0x77; ENTROPY_SEED_LEN]);
        assert_eq!(restore.snapshot, SNAPSHOT);
        assert_eq!(restore.entropy_seed, None);
        assert_eq!(fork.parent.slot_id, SlotId::new(1));
        assert_eq!(
            fork.entropy_seeds.len(),
            usize::try_from(fork.count).unwrap()
        );
    }

    #[test]
    fn hypervisor_inject_inputs_shapes_absolute_frame_and_pad_events() {
        let request = InjectInputsRequest {
            lease: sample_lease(2, 0x22),
            events: vec![ScheduledEvent {
                at: ScheduleAt::Frame(FrameCount::new(901)),
                event: InputEvent::PadSet(PadSet {
                    port: 0,
                    buttons: 0b1010,
                }),
            }],
        };
        let response = InjectInputsResponse { scheduled: 1 };

        assert_eq!(request.lease.slot_id, SlotId::new(2));
        assert_eq!(
            request.events[0].at,
            ScheduleAt::Frame(FrameCount::new(901))
        );
        assert!(matches!(request.events[0].event, InputEvent::PadSet(_)));
        assert_eq!(response.scheduled, 1);
    }

    #[test]
    fn hypervisor_run_frame_budget_reports_budget_reached() {
        let request = RunRequest {
            lease: sample_lease(3, 0x33),
            until: RunUntil::FrameBudget(FrameCount::new(4)),
            hard_icount_cap: Some(GuestInstructions::new(2_000_000)),
            capture: Some(sample_capture()),
        };
        let response = RunResponse {
            reason: StopReason::BudgetReached,
            icount: GuestInstructions::new(1_234_000),
            vns: 1_234_000,
            state_hash: STATE,
            frames_elapsed: 4,
            sdk_event: None,
            feature_bytes: Some(vec![1, 2, 3, 4]),
            fb_lz4: Some(vec![0xAA, 0xBB]),
            fb_info: Some(sample_fb_info()),
        };

        assert_eq!(request.until, RunUntil::FrameBudget(FrameCount::new(4)));
        assert_eq!(
            request.hard_icount_cap,
            Some(GuestInstructions::new(2_000_000))
        );
        assert_eq!(request.hard_icount_cap_wire_value(), 2_000_000);
        assert_eq!(response.reason, StopReason::BudgetReached);
        assert_eq!(response.frames_elapsed, 4);
        assert_eq!(
            response.fb_info.expect("fb info").frame_counter,
            FrameCount::new(904)
        );
    }

    #[test]
    fn hypervisor_snapshot_response_carries_digests_and_capture_meta() {
        let response = TakeSnapshotResponse {
            snapshot: SNAPSHOT,
            input_log_id: Some(LOG_ID),
            icount: GuestInstructions::new(9_000_000),
            vns: 9_000_000,
            state_hash: STATE,
            dirty_pages: 12,
            machine_config_hash: MACHINE_HASH,
            determinism_class: sample_class(),
            feature_bytes: Some(vec![5, 6, 7, 8]),
            fb_lz4: Some(vec![0xCC, 0xDD]),
            fb_info: Some(sample_fb_info()),
            frame_counter: FrameCount::new(904),
        };

        assert_eq!(response.snapshot, SNAPSHOT);
        assert_eq!(
            response.input_log_id.expect("sealed log").as_bytes(),
            &[0x44; INPUT_LOG_ID_LEN]
        );
        assert_eq!(response.state_hash, STATE);
        assert_eq!(response.machine_config_hash, MACHINE_HASH);
        assert_eq!(
            response.machine_config_hash.as_bytes(),
            &[0x33; DIGEST32_LEN]
        );
        assert_eq!(response.determinism_class.vmm_version, "dh-vmm 0.1.0");
        assert_eq!(
            response.fb_info.expect("fb info").format,
            PixelFormat::Xrgb8888
        );
        assert_eq!(response.frame_counter, FrameCount::new(904));
    }

    #[test]
    fn hypervisor_snapshot_response_can_represent_unsealed_log() {
        let response = TakeSnapshotResponse {
            snapshot: SNAPSHOT,
            input_log_id: None,
            icount: GuestInstructions::new(9_000_000),
            vns: 9_000_000,
            state_hash: STATE,
            dirty_pages: 12,
            machine_config_hash: MACHINE_HASH,
            determinism_class: sample_class(),
            feature_bytes: None,
            fb_lz4: None,
            fb_info: None,
            frame_counter: FrameCount::new(904),
        };

        assert_eq!(response.input_log_id, None);
        assert_eq!(response.frame_counter, FrameCount::new(904));
    }

    #[test]
    fn hypervisor_worker_info_carries_determinism_class() {
        let response = GetWorkerInfoResponse {
            worker_id: "worker-a".to_owned(),
            slots_total: 8,
            slots_free: 3,
            class: sample_class(),
            version: "dh-workerd 0.1.0".to_owned(),
        };

        assert_eq!(response.worker_id, "worker-a");
        assert_eq!(response.slots_total - response.slots_free, 5);
        assert_eq!(response.class.cpu_model, "family-6-model-85-stepping-7");
        assert_eq!(response.class.microcode, "0x5003605");
        assert_eq!(response.class.host_kernel, "6.8.0-kvm");
        assert_eq!(response.class.vmm_version, "dh-vmm 0.1.0");
    }

    #[test]
    fn hypervisor_slot_list_and_watch_surface_slot_identity() {
        let slot = SlotInfo {
            slot_id: SlotId::new(4),
            state: SlotState::Paused,
            icount: GuestInstructions::new(1000),
            base: Some(SNAPSHOT),
            live_children: 2,
        };
        let list = ListSlotsResponse { slots: vec![slot] };
        let watch = WatchSlotsResponse {
            events: vec![SlotEvent { slot }],
        };

        assert_eq!(list.slots[0].slot_id, SlotId::new(4));
        assert_eq!(list.slots[0].base, Some(SNAPSHOT));
        assert_eq!(watch.events[0].slot.state, SlotState::Paused);
        assert_eq!(watch.events[0].slot.live_children, 2);
    }

    #[test]
    fn hypervisor_wire_default_helpers_match_owner_scalar_defaults() {
        let request = RunRequest {
            lease: sample_lease(5, 0x55),
            until: RunUntil::Goal(GoalCondition {
                all_of: vec![MemPredicate {
                    gpa: 0x1000,
                    width: 4,
                    mask: u64::MAX,
                    op: PredicateOp::Eq,
                    value: 1,
                }],
                poll_period: None,
            }),
            hard_icount_cap: None,
            capture: None,
        };

        assert_eq!(request.hard_icount_cap_wire_value(), 0);
        let RunUntil::Goal(goal) = &request.until else {
            panic!("expected goal run");
        };
        assert_eq!(goal.poll_period_wire_value(), 0);

        let with_values = RunRequest {
            hard_icount_cap: Some(GuestInstructions::new(9_000)),
            until: RunUntil::Goal(GoalCondition {
                all_of: Vec::new(),
                poll_period: Some(GuestInstructions::new(1_000)),
            }),
            ..request
        };

        assert_eq!(with_values.hard_icount_cap_wire_value(), 9_000);
        let RunUntil::Goal(goal) = &with_values.until else {
            panic!("expected goal run");
        };
        assert_eq!(goal.poll_period_wire_value(), 1_000);
    }

    #[test]
    fn hypervisor_fork_request_validates_seed_count() {
        let valid = ForkRequest::new(
            sample_lease(6, 0x66),
            vec![EntropySeed::new([0x30; ENTROPY_SEED_LEN])],
        )
        .expect("valid fork request");

        assert_eq!(valid.count, 1);
        assert!(valid.validate().is_ok());

        let invalid = ForkRequest {
            parent: sample_lease(6, 0x66),
            count: 2,
            entropy_seeds: vec![EntropySeed::new([0x30; ENTROPY_SEED_LEN])],
        };
        let error = invalid.validate().expect_err("mismatched seed count");
        assert_eq!(error.kind(), ClientErrorKind::InvalidRequest);
    }

    #[test]
    fn hypervisor_enum_wire_order_is_stable() {
        assert_eq!(
            postcard::to_allocvec(&HashEpochs::EpochsOn).expect("hash epochs"),
            vec![1]
        );
        assert_eq!(
            postcard::to_allocvec(&PixelFormat::Xrgb8888).expect("pixel format"),
            vec![1]
        );
        assert_eq!(
            postcard::to_allocvec(&PredicateOp::Eq).expect("predicate op"),
            vec![1]
        );
        assert_eq!(
            postcard::to_allocvec(&StopReason::BudgetReached).expect("stop reason"),
            vec![1]
        );
        assert_eq!(
            postcard::to_allocvec(&SlotState::Paused).expect("slot state"),
            vec![2]
        );
    }

    #[test]
    fn hypervisor_dtos_round_trip_with_postcard() {
        let response = TakeSnapshotResponse {
            snapshot: SNAPSHOT,
            input_log_id: Some(LOG_ID),
            icount: GuestInstructions::new(9_000_000),
            vns: 9_000_000,
            state_hash: STATE,
            dirty_pages: 12,
            machine_config_hash: MACHINE_HASH,
            determinism_class: sample_class(),
            feature_bytes: Some(vec![5, 6, 7, 8]),
            fb_lz4: Some(vec![0xCC, 0xDD]),
            fb_info: Some(sample_fb_info()),
            frame_counter: FrameCount::new(904),
        };

        let encoded = postcard::to_allocvec(&response).expect("serialize snapshot response");
        let decoded: TakeSnapshotResponse =
            postcard::from_bytes(&encoded).expect("deserialize snapshot response");
        let encoded_again = postcard::to_allocvec(&decoded).expect("reserialize");

        assert_eq!(decoded, response);
        assert_eq!(encoded_again, encoded);
    }

    fn sample_lease(slot: u64, token_byte: u8) -> Lease {
        Lease {
            slot_id: SlotId::new(slot),
            token: LeaseToken::new([token_byte; LEASE_TOKEN_LEN]),
        }
    }

    fn sample_config() -> MachineConfig {
        MachineConfig {
            version: 1,
            mem_bytes: 128 * 1024 * 1024,
            vcpus: 1,
            clock_num: 1,
            clock_den: 1,
            base_image_hash: Digest32::new([0xAA; DIGEST32_LEN]),
            boot: BootSpec::Elf(ElfBoot {
                kernel_hash: Digest32::new([0xBB; DIGEST32_LEN]),
                cmdline: b"console=ttyS0".to_vec(),
            }),
            epoch_len: GuestInstructions::new(50_000_000),
            hash_epochs: HashEpochs::EpochsOn,
            skid_margin: 8192,
        }
    }

    fn sample_capture() -> CaptureSpec {
        CaptureSpec {
            ranges: vec![ExtractRange {
                region: "wram".to_owned(),
                layout_version: 2,
                offset: 0x1000,
                len: 64,
            }],
            framebuffer: true,
        }
    }

    fn sample_fb_info() -> FbInfo {
        FbInfo {
            width: 256,
            height: 224,
            stride: 1024,
            format: PixelFormat::Xrgb8888,
            frame_counter: FrameCount::new(904),
        }
    }

    fn sample_class() -> DeterminismClass {
        DeterminismClass {
            cpu_model: "family-6-model-85-stepping-7".to_owned(),
            microcode: "0x5003605".to_owned(),
            host_kernel: "6.8.0-kvm".to_owned(),
            vmm_version: "dh-vmm 0.1.0".to_owned(),
        }
    }
}
