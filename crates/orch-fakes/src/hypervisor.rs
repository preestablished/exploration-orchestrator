//! Fake hypervisor worker surface over the deterministic grid world.

use std::collections::BTreeMap;

use orch_clients::{
    hypervisor::{
        CaptureSpec, CreateVmRequest, CreateVmResponse, DestroyVmRequest, DestroyVmResponse,
        DeterminismClass, Digest32, EntropySeed, FbInfo, ForkRequest, ForkResponse,
        GetWorkerInfoRequest, GetWorkerInfoResponse, GuestEvent, HypervisorWorkerClient,
        InjectInputsRequest, InjectInputsResponse, InputEvent, InputLogId, Lease, LeaseToken,
        ListSlotsRequest, ListSlotsResponse, MachineConfig, PixelFormat, RestoreSnapshotRequest,
        RestoreSnapshotResponse, RunRequest, RunResponse, RunUntil, ScheduleAt, ScheduledEvent,
        SlotEvent, SlotId, SlotInfo, SlotState, StopReason, TakeSnapshotRequest,
        TakeSnapshotResponse, WatchSlotsRequest, WatchSlotsResponse, DIGEST32_LEN, LEASE_TOKEN_LEN,
    },
    ClientError, ClientErrorKind, ClientResult,
};
use orch_core::types::{FrameCount, GuestInstructions, SnapshotRef};

use crate::{
    grid::{GridAction, GridState},
    scorer::encode_grid_features,
};

pub const DEFAULT_SLOTS_TOTAL: u32 = 8;
pub const FAKE_HYPERVISOR_VERSION: &str = "fake-hypervisor/0.1";

const ICOUNT_PER_FRAME: u64 = 1_000;
const VNS_PER_FRAME: u64 = 16_666_667;
const READY_PAYLOAD: &[u8] = b"Ready";

const BUTTON_ATTACK_A: u32 = 0b0000_0001;
const BUTTON_ATTACK_B: u32 = 0b0000_0010;
const BUTTON_ATTACK_Y: u32 = 0b0000_1000;
const BUTTON_UP: u32 = 0b0100_0000;
const BUTTON_DOWN: u32 = 0b1000_0000;
const BUTTON_LEFT: u32 = 0b1_0000_0000;
const BUTTON_RIGHT: u32 = 0b10_0000_0000;

#[derive(Clone, Debug, PartialEq)]
pub struct FakeHypervisor {
    worker_id: String,
    slots_total: u32,
    class: DeterminismClass,
    next_slot_id: u64,
    slots: BTreeMap<SlotId, Slot>,
    snapshots: BTreeMap<SnapshotRef, SnapshotRecord>,
    watch_events: Vec<SlotEvent>,
}

impl Default for FakeHypervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeHypervisor {
    #[must_use]
    pub fn new() -> Self {
        Self::with_slots(DEFAULT_SLOTS_TOTAL)
    }

    #[must_use]
    pub fn with_slots(slots_total: u32) -> Self {
        Self {
            worker_id: "fake-grid-worker-0".to_owned(),
            slots_total,
            class: fake_determinism_class(),
            next_slot_id: 1,
            slots: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            watch_events: Vec::new(),
        }
    }

    #[must_use]
    pub fn deterministic_class(&self) -> &DeterminismClass {
        &self.class
    }

    fn ensure_capacity(&self, needed: u32) -> ClientResult<()> {
        let used = u32::try_from(self.slots.len()).map_err(|_| {
            ClientError::new(ClientErrorKind::Internal, "active slot count exceeds u32")
        })?;
        if self.slots_total.saturating_sub(used) < needed {
            return Err(ClientError::new(
                ClientErrorKind::ResourceExhausted,
                "fake hypervisor slot capacity exhausted",
            ));
        }
        Ok(())
    }

    fn allocate_lease(&mut self, config: &MachineConfig, entropy_seed: EntropySeed) -> Lease {
        let slot_id = SlotId::new(self.next_slot_id);
        self.next_slot_id = self.next_slot_id.saturating_add(1);
        let token = lease_token(slot_id, config, entropy_seed);
        Lease { slot_id, token }
    }

    fn slot(&self, lease: Lease) -> ClientResult<&Slot> {
        let slot = self.slots.get(&lease.slot_id).ok_or_else(|| {
            ClientError::new(
                ClientErrorKind::NotFound,
                format!("unknown slot {}", lease.slot_id.get()),
            )
        })?;
        if slot.lease.token != lease.token {
            return Err(ClientError::new(
                ClientErrorKind::FailedPrecondition,
                "lease token does not match slot",
            ));
        }
        Ok(slot)
    }

    fn slot_mut(&mut self, lease: Lease) -> ClientResult<&mut Slot> {
        let slot = self.slots.get_mut(&lease.slot_id).ok_or_else(|| {
            ClientError::new(
                ClientErrorKind::NotFound,
                format!("unknown slot {}", lease.slot_id.get()),
            )
        })?;
        if slot.lease.token != lease.token {
            return Err(ClientError::new(
                ClientErrorKind::FailedPrecondition,
                "lease token does not match slot",
            ));
        }
        Ok(slot)
    }

    fn push_slot_event(&mut self, slot_id: SlotId) {
        if let Some(slot) = self.slots.get(&slot_id) {
            self.watch_events.push(SlotEvent {
                slot: self.slot_info(slot),
            });
        }
    }

    fn slot_info(&self, slot: &Slot) -> SlotInfo {
        let live_children = self
            .slots
            .values()
            .filter(|candidate| candidate.parent == Some(slot.lease.slot_id))
            .count()
            .try_into()
            .unwrap_or(u32::MAX);
        SlotInfo {
            slot_id: slot.lease.slot_id,
            state: slot.state_kind,
            icount: slot.icount,
            base: slot.base,
            live_children,
        }
    }
}

impl HypervisorWorkerClient for FakeHypervisor {
    fn create_vm(&mut self, request: CreateVmRequest) -> ClientResult<CreateVmResponse> {
        self.ensure_capacity(1)?;
        let lease = self.allocate_lease(&request.config, request.entropy_seed);
        let slot = Slot::new_root(lease, request.config);
        self.slots.insert(lease.slot_id, slot);
        self.push_slot_event(lease.slot_id);

        Ok(CreateVmResponse {
            lease,
            icount: GuestInstructions::new(0),
        })
    }

    fn restore_snapshot(
        &mut self,
        request: RestoreSnapshotRequest,
    ) -> ClientResult<RestoreSnapshotResponse> {
        self.ensure_capacity(1)?;
        let record = self
            .snapshots
            .get(&request.snapshot)
            .cloned()
            .ok_or_else(|| ClientError::new(ClientErrorKind::NotFound, "snapshot not found"))?;
        let entropy_seed = request.entropy_seed.unwrap_or_else(zero_entropy_seed);
        let lease = self.allocate_lease(&record.config, entropy_seed);
        let slot = Slot::from_snapshot(lease, request.snapshot, &record);
        self.slots.insert(lease.slot_id, slot);
        self.push_slot_event(lease.slot_id);

        Ok(RestoreSnapshotResponse {
            lease,
            config: record.config,
            state_hash: record.state.state_hash(),
            frame_counter: record.frame_counter,
        })
    }

    fn fork(&mut self, request: ForkRequest) -> ClientResult<ForkResponse> {
        request.validate()?;
        self.ensure_capacity(request.count)?;
        let parent = self.slot(request.parent)?.clone();
        let mut children = Vec::with_capacity(request.entropy_seeds.len());

        for entropy_seed in request.entropy_seeds {
            let lease = self.allocate_lease(&parent.config, entropy_seed);
            let child = parent.clone_as_child(lease, request.parent.slot_id);
            self.slots.insert(lease.slot_id, child);
            self.push_slot_event(lease.slot_id);
            children.push(lease);
        }
        self.push_slot_event(request.parent.slot_id);

        Ok(ForkResponse { children })
    }

    fn inject_inputs(
        &mut self,
        request: InjectInputsRequest,
    ) -> ClientResult<InjectInputsResponse> {
        let scheduled = u32::try_from(request.events.len()).map_err(|_| {
            ClientError::new(ClientErrorKind::InvalidRequest, "too many scheduled events")
        })?;
        let slot = self.slot_mut(request.lease)?;

        for event in request.events {
            let action = event_to_action(&event.event);
            let pending = PendingAction {
                at_frame: event_frame(&event),
                order: slot.next_input_order,
                action,
            };
            slot.next_input_order = slot.next_input_order.checked_add(1).ok_or_else(|| {
                ClientError::new(ClientErrorKind::Internal, "input order overflow")
            })?;
            slot.input_log.push(event);
            slot.pending_actions.push(pending);
        }
        slot.pending_actions
            .sort_by_key(|pending| (pending.at_frame.get(), pending.order));

        Ok(InjectInputsResponse { scheduled })
    }

    fn run(&mut self, request: RunRequest) -> ClientResult<RunResponse> {
        let capture = request.capture;
        let slot = self.slot_mut(request.lease)?;
        slot.state_kind = SlotState::Running;
        let (reason, frames_elapsed, sdk_event) =
            run_slot(slot, request.until, request.hard_icount_cap)?;
        slot.state_kind = SlotState::Paused;
        let capture = capture_response(slot.state, slot.frame_counter, capture.as_ref());

        Ok(RunResponse {
            reason,
            icount: slot.icount,
            vns: slot.vns,
            state_hash: slot.state.state_hash(),
            frames_elapsed,
            sdk_event,
            feature_bytes: capture.feature_bytes,
            fb_lz4: capture.fb_lz4,
            fb_info: capture.fb_info,
        })
    }

    fn take_snapshot(
        &mut self,
        request: TakeSnapshotRequest,
    ) -> ClientResult<TakeSnapshotResponse> {
        let slot = self.slot(request.lease)?.clone();
        let machine_config_hash = machine_config_hash(&slot.config);
        let input_log_id = request
            .seal_input_log
            .then(|| input_log_id(&slot.input_log));
        let snapshot = snapshot_ref(
            &slot.config,
            slot.state,
            slot.icount,
            slot.vns,
            slot.frame_counter,
            &slot.input_log,
        );
        let capture = capture_response(slot.state, slot.frame_counter, request.capture.as_ref());
        let dirty_pages = 1u32
            .checked_add(u32::try_from(slot.input_log.len()).unwrap_or(u32::MAX - 1))
            .unwrap_or(u32::MAX);

        self.snapshots.insert(
            snapshot,
            SnapshotRecord {
                config: slot.config.clone(),
                state: slot.state,
                icount: slot.icount,
                frame_counter: slot.frame_counter,
                vns: slot.vns,
                input_log: slot.input_log.clone(),
                pending_actions: slot.pending_actions.clone(),
                next_input_order: slot.next_input_order,
                machine_config_hash,
            },
        );

        Ok(TakeSnapshotResponse {
            snapshot,
            input_log_id,
            icount: slot.icount,
            vns: slot.vns,
            state_hash: slot.state.state_hash(),
            dirty_pages,
            machine_config_hash,
            determinism_class: self.class.clone(),
            feature_bytes: capture.feature_bytes,
            fb_lz4: capture.fb_lz4,
            fb_info: capture.fb_info,
            frame_counter: slot.frame_counter,
        })
    }

    fn destroy_vm(&mut self, request: DestroyVmRequest) -> ClientResult<DestroyVmResponse> {
        let slot = self.slot(request.lease)?.clone();
        self.slots.remove(&request.lease.slot_id);
        self.watch_events.push(SlotEvent {
            slot: SlotInfo {
                slot_id: slot.lease.slot_id,
                state: SlotState::Empty,
                icount: slot.icount,
                base: slot.base,
                live_children: 0,
            },
        });
        Ok(DestroyVmResponse)
    }

    fn list_slots(&self, _request: ListSlotsRequest) -> ClientResult<ListSlotsResponse> {
        Ok(ListSlotsResponse {
            slots: self
                .slots
                .values()
                .map(|slot| self.slot_info(slot))
                .collect(),
        })
    }

    fn watch_slots(&self, _request: WatchSlotsRequest) -> ClientResult<WatchSlotsResponse> {
        Ok(WatchSlotsResponse {
            events: self.watch_events.clone(),
        })
    }

    fn worker_info(&self, _request: GetWorkerInfoRequest) -> ClientResult<GetWorkerInfoResponse> {
        let used = u32::try_from(self.slots.len()).unwrap_or(u32::MAX);
        Ok(GetWorkerInfoResponse {
            worker_id: self.worker_id.clone(),
            slots_total: self.slots_total,
            slots_free: self.slots_total.saturating_sub(used),
            class: self.class.clone(),
            version: FAKE_HYPERVISOR_VERSION.to_owned(),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Slot {
    lease: Lease,
    config: MachineConfig,
    state: GridState,
    icount: GuestInstructions,
    frame_counter: FrameCount,
    vns: u64,
    base: Option<SnapshotRef>,
    parent: Option<SlotId>,
    pending_actions: Vec<PendingAction>,
    input_log: Vec<ScheduledEvent>,
    next_input_order: u64,
    state_kind: SlotState,
}

impl Slot {
    fn new_root(lease: Lease, config: MachineConfig) -> Self {
        Self {
            lease,
            config,
            state: GridState::new(),
            icount: GuestInstructions::new(0),
            frame_counter: FrameCount::new(0),
            vns: 0,
            base: None,
            parent: None,
            pending_actions: Vec::new(),
            input_log: Vec::new(),
            next_input_order: 0,
            state_kind: SlotState::Paused,
        }
    }

    fn from_snapshot(lease: Lease, snapshot: SnapshotRef, record: &SnapshotRecord) -> Self {
        Self {
            lease,
            config: record.config.clone(),
            state: record.state,
            icount: record.icount,
            frame_counter: record.frame_counter,
            vns: record.vns,
            base: Some(snapshot),
            parent: None,
            pending_actions: record.pending_actions.clone(),
            input_log: record.input_log.clone(),
            next_input_order: record.next_input_order,
            state_kind: SlotState::Paused,
        }
    }

    fn clone_as_child(&self, lease: Lease, parent: SlotId) -> Self {
        Self {
            lease,
            config: self.config.clone(),
            state: self.state,
            icount: self.icount,
            frame_counter: self.frame_counter,
            vns: self.vns,
            base: self.base,
            parent: Some(parent),
            pending_actions: self.pending_actions.clone(),
            input_log: self.input_log.clone(),
            next_input_order: self.next_input_order,
            state_kind: SlotState::Paused,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct PendingAction {
    at_frame: FrameCount,
    order: u64,
    action: GridAction,
}

#[derive(Clone, Debug, PartialEq)]
struct SnapshotRecord {
    config: MachineConfig,
    state: GridState,
    icount: GuestInstructions,
    frame_counter: FrameCount,
    vns: u64,
    input_log: Vec<ScheduledEvent>,
    pending_actions: Vec<PendingAction>,
    next_input_order: u64,
    machine_config_hash: Digest32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CaptureResponse {
    feature_bytes: Option<Vec<u8>>,
    fb_lz4: Option<Vec<u8>>,
    fb_info: Option<FbInfo>,
}

fn run_slot(
    slot: &mut Slot,
    until: RunUntil,
    hard_icount_cap: Option<GuestInstructions>,
) -> ClientResult<(StopReason, u64, Option<GuestEvent>)> {
    match until {
        RunUntil::NextSdkEvent(next) => {
            let sdk_event = GuestEvent {
                stream: next.stream.unwrap_or(0),
                icount: slot.icount,
                vns: slot.vns,
                payload: READY_PAYLOAD.to_vec(),
            };
            Ok((StopReason::NextSdkEvent, 0, Some(sdk_event)))
        }
        RunUntil::FrameBudget(frames) => {
            let (frames_elapsed, reason) = advance_frames(slot, frames.get(), hard_icount_cap)?;
            Ok((reason, u64::from(frames_elapsed), None))
        }
        RunUntil::IcountBudget(budget) => {
            let frame_count = budget
                .get()
                .div_ceil(ICOUNT_PER_FRAME)
                .min(u64::from(u32::MAX));
            let (frames_elapsed, reason) =
                advance_frames(slot, frame_count as u32, hard_icount_cap)?;
            Ok((reason, u64::from(frames_elapsed), None))
        }
        RunUntil::VnsBudget(budget) => {
            let frame_count = budget.div_ceil(VNS_PER_FRAME).min(u64::from(u32::MAX));
            let (frames_elapsed, reason) =
                advance_frames(slot, frame_count as u32, hard_icount_cap)?;
            Ok((reason, u64::from(frames_elapsed), None))
        }
        RunUntil::Goal(_) if slot.state.goal_reached() => Ok((StopReason::GoalSatisfied, 0, None)),
        RunUntil::Goal(_) => {
            let (frames_elapsed, reason) = advance_frames(slot, 1, hard_icount_cap)?;
            let reason = if slot.state.goal_reached() {
                StopReason::GoalSatisfied
            } else {
                reason
            };
            Ok((reason, u64::from(frames_elapsed), None))
        }
    }
}

fn advance_frames(
    slot: &mut Slot,
    requested_frames: u32,
    hard_icount_cap: Option<GuestInstructions>,
) -> ClientResult<(u32, StopReason)> {
    let mut frames = requested_frames;
    let mut reason = StopReason::BudgetReached;
    if let Some(cap) = hard_icount_cap {
        let current = slot.icount.get();
        if cap.get() <= current {
            frames = 0;
            reason = StopReason::HardCap;
        } else {
            let remaining = cap.get() - current;
            let requested_icount = u64::from(requested_frames).saturating_mul(ICOUNT_PER_FRAME);
            if requested_icount > remaining {
                frames = (remaining / ICOUNT_PER_FRAME).min(u64::from(u32::MAX)) as u32;
                reason = StopReason::HardCap;
            }
        }
    }

    let start_frame = slot.frame_counter.get();
    let end_frame = start_frame
        .checked_add(frames)
        .ok_or_else(|| ClientError::new(ClientErrorKind::Internal, "frame counter overflow"))?;
    let due_actions = drain_due_actions(&mut slot.pending_actions, end_frame);
    for pending in due_actions {
        slot.state = slot.state.step(pending.action).0;
    }
    slot.frame_counter = FrameCount::new(end_frame);
    slot.icount = GuestInstructions::new(
        slot.icount
            .get()
            .checked_add(u64::from(frames).saturating_mul(ICOUNT_PER_FRAME))
            .ok_or_else(|| ClientError::new(ClientErrorKind::Internal, "icount overflow"))?,
    );
    slot.vns = slot
        .vns
        .checked_add(u64::from(frames).saturating_mul(VNS_PER_FRAME))
        .ok_or_else(|| ClientError::new(ClientErrorKind::Internal, "vns overflow"))?;

    Ok((frames, reason))
}

fn drain_due_actions(
    pending_actions: &mut Vec<PendingAction>,
    end_frame: u32,
) -> Vec<PendingAction> {
    let mut due = Vec::new();
    let mut remaining = Vec::with_capacity(pending_actions.len());
    for pending in pending_actions.drain(..) {
        if pending.at_frame.get() <= end_frame {
            due.push(pending);
        } else {
            remaining.push(pending);
        }
    }
    due.sort_by_key(|pending| (pending.at_frame.get(), pending.order));
    *pending_actions = remaining;
    due
}

fn capture_response(
    state: GridState,
    frame_counter: FrameCount,
    capture: Option<&CaptureSpec>,
) -> CaptureResponse {
    let Some(capture) = capture else {
        return CaptureResponse::default();
    };

    CaptureResponse {
        feature_bytes: Some(pack_capture_ranges(state, capture)),
        fb_lz4: capture
            .framebuffer
            .then(|| inline_framebuffer(state, frame_counter)),
        fb_info: capture.framebuffer.then_some(FbInfo {
            width: 1,
            height: 1,
            stride: 4,
            format: PixelFormat::Xrgb8888,
            frame_counter,
        }),
    }
}

fn pack_capture_ranges(state: GridState, capture: &CaptureSpec) -> Vec<u8> {
    let grid = encode_grid_features(state);
    let mut packed = Vec::new();
    for range in &capture.ranges {
        let len = range.len as usize;
        let start = usize::try_from(range.offset).unwrap_or(usize::MAX);
        let mut bytes = vec![0; len];
        if range.region == "grid" && start < grid.len() {
            let available = grid.len() - start;
            let copy_len = available.min(len);
            bytes[..copy_len].copy_from_slice(&grid[start..start + copy_len]);
        }
        packed.extend_from_slice(&bytes);
    }
    packed
}

fn inline_framebuffer(state: GridState, frame_counter: FrameCount) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16);
    bytes.extend_from_slice(b"gridfb1");
    bytes.extend_from_slice(&frame_counter.get().to_le_bytes());
    bytes.extend_from_slice(&encode_grid_features(state));
    bytes
}

fn event_frame(event: &ScheduledEvent) -> FrameCount {
    match event.at {
        ScheduleAt::Frame(frame) => frame,
        ScheduleAt::Icount(icount) => {
            FrameCount::new((icount.get() / ICOUNT_PER_FRAME).min(u64::from(u32::MAX)) as u32)
        }
        ScheduleAt::Vns(vns) => {
            FrameCount::new((vns / VNS_PER_FRAME).min(u64::from(u32::MAX)) as u32)
        }
    }
}

fn event_to_action(event: &InputEvent) -> GridAction {
    let InputEvent::PadSet(pad) = event else {
        return GridAction::Wait;
    };
    let buttons = pad.buttons;
    if buttons & BUTTON_RIGHT != 0 {
        GridAction::Right
    } else if buttons & BUTTON_LEFT != 0 {
        GridAction::Left
    } else if buttons & BUTTON_UP != 0 {
        GridAction::Up
    } else if buttons & BUTTON_DOWN != 0 {
        GridAction::Down
    } else if buttons & (BUTTON_ATTACK_A | BUTTON_ATTACK_B | BUTTON_ATTACK_Y) != 0 {
        GridAction::Attack
    } else {
        GridAction::Wait
    }
}

fn fake_determinism_class() -> DeterminismClass {
    DeterminismClass {
        cpu_model: "family-6-model-85-stepping-7".to_owned(),
        microcode: "0x5003605".to_owned(),
        host_kernel: "6.8.0-kvm".to_owned(),
        vmm_version: FAKE_HYPERVISOR_VERSION.to_owned(),
    }
}

fn zero_entropy_seed() -> EntropySeed {
    EntropySeed::new([0; 32])
}

fn lease_token(slot_id: SlotId, config: &MachineConfig, entropy_seed: EntropySeed) -> LeaseToken {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"orch-fakes/hypervisor/lease-token/v1");
    hasher.update(&slot_id.get().to_le_bytes());
    hasher.update(machine_config_hash(config).as_bytes());
    hasher.update(entropy_seed.as_bytes());
    let hash = hasher.finalize();
    let mut token = [0; LEASE_TOKEN_LEN];
    token.copy_from_slice(&hash.as_bytes()[..LEASE_TOKEN_LEN]);
    LeaseToken::new(token)
}

fn machine_config_hash(config: &MachineConfig) -> Digest32 {
    digest32(b"orch-fakes/hypervisor/machine-config/v1", config)
}

fn input_log_id(input_log: &[ScheduledEvent]) -> InputLogId {
    InputLogId::new(digest_bytes(
        b"orch-fakes/hypervisor/input-log/v1",
        &postcard::to_allocvec(input_log).expect("input log serializes"),
    ))
}

fn snapshot_ref(
    config: &MachineConfig,
    state: GridState,
    icount: GuestInstructions,
    vns: u64,
    frame_counter: FrameCount,
    input_log: &[ScheduledEvent],
) -> SnapshotRef {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"orch-fakes/hypervisor/snapshot/v1");
    hasher.update(machine_config_hash(config).as_bytes());
    hasher.update(state.state_hash().as_bytes());
    hasher.update(&icount.get().to_le_bytes());
    hasher.update(&vns.to_le_bytes());
    hasher.update(&frame_counter.get().to_le_bytes());
    hasher.update(input_log_id(input_log).as_bytes());
    SnapshotRef::new(*hasher.finalize().as_bytes())
}

fn digest32<T: serde::Serialize>(domain: &[u8], value: &T) -> Digest32 {
    Digest32::new(digest_bytes(
        domain,
        &postcard::to_allocvec(value).expect("value serializes"),
    ))
}

fn digest_bytes(domain: &[u8], bytes: &[u8]) -> [u8; DIGEST32_LEN] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_clients::hypervisor::{
        BootSpec, CaptureSpec, ElfBoot, ExtractRange, HashEpochs, NextSdkEvent, PadSet,
    };

    #[test]
    fn hypervisor_bootstrap_ready_and_worker_info_are_deterministic() {
        let mut fake = FakeHypervisor::with_slots(2);
        let created = fake
            .create_vm(CreateVmRequest {
                config: sample_config(),
                entropy_seed: EntropySeed::new([0x10; 32]),
            })
            .expect("create vm");

        let ready = fake
            .run(RunRequest {
                lease: created.lease,
                until: RunUntil::NextSdkEvent(NextSdkEvent { stream: Some(7) }),
                hard_icount_cap: None,
                capture: None,
            })
            .expect("ready event");
        let info = fake.worker_info(GetWorkerInfoRequest).expect("worker info");

        assert_eq!(ready.reason, StopReason::NextSdkEvent);
        let event = ready.sdk_event.expect("sdk event");
        assert_eq!(event.stream, 7);
        assert_eq!(event.payload, READY_PAYLOAD);
        assert_eq!(info.slots_total, 2);
        assert_eq!(info.slots_free, 1);
        assert_eq!(info.class, *fake.deterministic_class());
        assert_eq!(info.class.vmm_version, FAKE_HYPERVISOR_VERSION);
    }

    #[test]
    fn hypervisor_frame_budget_applies_inputs_and_snapshot_captures_inline_data() {
        let mut fake = FakeHypervisor::new();
        let created = create_sample_vm(&mut fake, 0x20);
        fake.inject_inputs(InjectInputsRequest {
            lease: created.lease,
            events: vec![pad_event(1, BUTTON_RIGHT)],
        })
        .expect("inject");

        let capture = grid_capture(true);
        let run = fake
            .run(RunRequest {
                lease: created.lease,
                until: RunUntil::FrameBudget(FrameCount::new(1)),
                hard_icount_cap: None,
                capture: Some(capture.clone()),
            })
            .expect("run");
        let snapshot = fake
            .take_snapshot(TakeSnapshotRequest {
                lease: created.lease,
                seal_input_log: true,
                capture: Some(capture),
            })
            .expect("snapshot");

        assert_eq!(run.reason, StopReason::BudgetReached);
        assert_eq!(run.frames_elapsed, 1);
        assert_eq!(run.icount, GuestInstructions::new(ICOUNT_PER_FRAME));
        assert_eq!(run.feature_bytes.expect("features"), vec![0, 1, 2, 0, 3]);
        assert_eq!(
            run.fb_info.expect("fb info").frame_counter,
            FrameCount::new(1)
        );
        assert!(run.fb_lz4.expect("fb bytes").starts_with(b"gridfb1"));
        assert!(snapshot.input_log_id.is_some());
        assert_eq!(
            snapshot.state_hash,
            GridState::new().step(GridAction::Right).0.state_hash()
        );
        assert_eq!(
            snapshot.machine_config_hash,
            machine_config_hash(&sample_config())
        );
        assert_eq!(snapshot.determinism_class, fake_determinism_class());
        assert_eq!(snapshot.frame_counter, FrameCount::new(1));
        assert!(snapshot
            .fb_lz4
            .expect("snapshot fb")
            .starts_with(b"gridfb1"));
    }

    #[test]
    fn hypervisor_restore_and_fork_equivalence_uses_deterministic_snapshots() {
        let mut fake = FakeHypervisor::new();
        let created = create_sample_vm(&mut fake, 0x30);
        fake.inject_inputs(InjectInputsRequest {
            lease: created.lease,
            events: vec![pad_event(1, BUTTON_RIGHT), pad_event(2, BUTTON_RIGHT)],
        })
        .expect("inject");
        fake.run(RunRequest {
            lease: created.lease,
            until: RunUntil::FrameBudget(FrameCount::new(2)),
            hard_icount_cap: None,
            capture: None,
        })
        .expect("run parent");
        let parent_snapshot = fake
            .take_snapshot(TakeSnapshotRequest {
                lease: created.lease,
                seal_input_log: true,
                capture: None,
            })
            .expect("parent snapshot");

        let restored = fake
            .restore_snapshot(RestoreSnapshotRequest {
                snapshot: parent_snapshot.snapshot,
                entropy_seed: Some(EntropySeed::new([0x31; 32])),
            })
            .expect("restore");
        let forked = fake
            .fork(ForkRequest::new(restored.lease, vec![EntropySeed::new([0x32; 32])]).unwrap())
            .expect("fork")
            .children
            .pop()
            .expect("child");

        let restored_snapshot = fake
            .take_snapshot(TakeSnapshotRequest {
                lease: restored.lease,
                seal_input_log: true,
                capture: Some(grid_capture(false)),
            })
            .expect("restored snapshot");
        let fork_snapshot = fake
            .take_snapshot(TakeSnapshotRequest {
                lease: forked,
                seal_input_log: true,
                capture: Some(grid_capture(false)),
            })
            .expect("fork snapshot");

        assert_eq!(restored.state_hash, parent_snapshot.state_hash);
        assert_eq!(restored.frame_counter, FrameCount::new(2));
        assert_eq!(restored_snapshot.snapshot, fork_snapshot.snapshot);
        assert_eq!(restored_snapshot.state_hash, fork_snapshot.state_hash);
        assert_eq!(restored_snapshot.frame_counter, fork_snapshot.frame_counter);
        assert_eq!(restored_snapshot.feature_bytes, fork_snapshot.feature_bytes);
    }

    #[test]
    fn hypervisor_slot_lifecycle_list_watch_and_capacity_are_stable() {
        let mut fake = FakeHypervisor::with_slots(2);
        let first = create_sample_vm(&mut fake, 0x40);
        let second = create_sample_vm(&mut fake, 0x41);
        let error = fake
            .create_vm(CreateVmRequest {
                config: sample_config(),
                entropy_seed: EntropySeed::new([0x42; 32]),
            })
            .expect_err("capacity");

        assert_eq!(error.kind(), ClientErrorKind::ResourceExhausted);
        let listed = fake.list_slots(ListSlotsRequest).expect("list");
        assert_eq!(listed.slots.len(), 2);
        assert_eq!(listed.slots[0].slot_id, first.lease.slot_id);
        assert_eq!(listed.slots[1].slot_id, second.lease.slot_id);
        assert!(listed
            .slots
            .iter()
            .all(|slot| slot.state == SlotState::Paused));

        fake.destroy_vm(DestroyVmRequest { lease: first.lease })
            .expect("destroy");
        let after_destroy = fake.list_slots(ListSlotsRequest).expect("list after");
        let watched = fake.watch_slots(WatchSlotsRequest).expect("watch");

        assert_eq!(after_destroy.slots.len(), 1);
        assert_eq!(after_destroy.slots[0].slot_id, second.lease.slot_id);
        assert_eq!(watched.events.len(), 3);
        assert_eq!(
            watched.events.last().expect("destroy event").slot.state,
            SlotState::Empty
        );
    }

    #[test]
    fn hypervisor_capture_packs_grid_ranges_in_request_order() {
        let mut fake = FakeHypervisor::new();
        let created = create_sample_vm(&mut fake, 0x50);
        let capture = CaptureSpec {
            ranges: vec![
                ExtractRange {
                    region: "grid".to_owned(),
                    layout_version: 1,
                    offset: 1,
                    len: 2,
                },
                ExtractRange {
                    region: "grid".to_owned(),
                    layout_version: 1,
                    offset: 0,
                    len: 1,
                },
                ExtractRange {
                    region: "grid".to_owned(),
                    layout_version: 1,
                    offset: 4,
                    len: 1,
                },
            ],
            framebuffer: false,
        };

        let snapshot = fake
            .take_snapshot(TakeSnapshotRequest {
                lease: created.lease,
                seal_input_log: false,
                capture: Some(capture),
            })
            .expect("snapshot");

        assert_eq!(snapshot.feature_bytes, Some(vec![0, 2, 0, 3]));
        assert_eq!(snapshot.fb_lz4, None);
        assert_eq!(snapshot.fb_info, None);
    }

    #[test]
    fn hypervisor_repeated_snapshots_are_stable_without_state_changes() {
        let mut fake = FakeHypervisor::new();
        let created = create_sample_vm(&mut fake, 0x60);

        let first = fake
            .take_snapshot(TakeSnapshotRequest {
                lease: created.lease,
                seal_input_log: true,
                capture: Some(grid_capture(true)),
            })
            .expect("first");
        let second = fake
            .take_snapshot(TakeSnapshotRequest {
                lease: created.lease,
                seal_input_log: true,
                capture: Some(grid_capture(true)),
            })
            .expect("second");

        assert_eq!(first.snapshot, second.snapshot);
        assert_eq!(first.input_log_id, second.input_log_id);
        assert_eq!(first.feature_bytes, second.feature_bytes);
        assert_eq!(first.fb_lz4, second.fb_lz4);
    }

    fn create_sample_vm(fake: &mut FakeHypervisor, seed_byte: u8) -> CreateVmResponse {
        fake.create_vm(CreateVmRequest {
            config: sample_config(),
            entropy_seed: EntropySeed::new([seed_byte; 32]),
        })
        .expect("create vm")
    }

    fn pad_event(frame: u32, buttons: u32) -> ScheduledEvent {
        ScheduledEvent {
            at: ScheduleAt::Frame(FrameCount::new(frame)),
            event: InputEvent::PadSet(PadSet { port: 0, buttons }),
        }
    }

    fn grid_capture(framebuffer: bool) -> CaptureSpec {
        CaptureSpec {
            ranges: vec![ExtractRange {
                region: "grid".to_owned(),
                layout_version: 1,
                offset: 0,
                len: 5,
            }],
            framebuffer,
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
}
