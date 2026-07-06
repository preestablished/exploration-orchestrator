//! `SlotView`: the scheduler's local view of worker slot capacity.
//!
//! Seeded from `GetWorkerInfo` + `ListSlots` at startup and maintained by a
//! periodic virtual-time drain of `WatchSlots` (the fakes' incremental cursor
//! model; a real transport at M6 swaps in a server stream behind the same
//! port). The view is advisory: `acquire` suspends until a slot is *likely*
//! free, while the worker itself stays the capacity authority —
//! `RESOURCE_EXHAUSTED` on an actual RPC is backpressure (release and
//! retry), never an error.
//!
//! Pool shrink/grow surfaces through `GetWorkerInfo::slots_total`, refreshed
//! on every drain; `WatchSlots` transitions keep the live-slot mirror
//! current so externally created or destroyed slots adjust the free view
//! mid-run.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use orch_clients::{
    hypervisor::{
        DeterminismClass, GetWorkerInfoRequest, ListSlotsRequest, SlotId, SlotState,
        WatchSlotsRequest,
    },
    ClientError, ClientErrorKind, ClientResult,
};
use tokio::{sync::oneshot, task::JoinHandle, time::Instant};

use crate::ports::AsyncHypervisor;

/// Grep-able FAILED-reason prefix for determinism-class refusals (M5
/// runbook forward-design; keep the string stable).
pub const CLASS_MISMATCH_REASON: &str = "determinism-class-mismatch";

#[derive(Clone, Debug)]
pub struct SlotViewConfig {
    /// Virtual-time interval between `WatchSlots` drains.
    pub drain_interval: Duration,
    /// Permit dispatch onto a worker whose determinism class differs from
    /// the job's requirement (default false).
    pub allow_class_mismatch: bool,
}

impl Default for SlotViewConfig {
    fn default() -> Self {
        Self {
            drain_interval: Duration::from_millis(5),
            allow_class_mismatch: false,
        }
    }
}

#[derive(Debug)]
struct ViewState {
    capacity: u32,
    reserved: u32,
    known: BTreeMap<SlotId, SlotState>,
    waiters: VecDeque<oneshot::Sender<()>>,
    worker_class: DeterminismClass,
    busy_integral: Duration,
    capacity_integral: Duration,
    last_advance: Instant,
}

impl ViewState {
    fn live(&self) -> u32 {
        u32::try_from(
            self.known
                .values()
                .filter(|state| !matches!(state, SlotState::Empty))
                .count(),
        )
        .unwrap_or(u32::MAX)
    }

    /// Slots the scheduler should treat as occupied: reservations it handed
    /// out, or live slots the worker reports (whichever is larger, so a
    /// frozen fork-parent or an externally created slot counts as busy).
    fn busy(&self) -> u32 {
        self.reserved.max(self.live())
    }

    fn free(&self) -> u32 {
        self.capacity.saturating_sub(self.busy())
    }

    /// Accumulates the busy-slot and capacity integrals up to `now`.
    fn advance_integrals(&mut self, now: Instant) {
        let dt = now.saturating_duration_since(self.last_advance);
        if dt > Duration::ZERO {
            self.busy_integral += dt * self.busy().min(self.capacity);
            self.capacity_integral += dt * self.capacity;
            self.last_advance = now;
        }
    }

    fn wake_free_waiters(&mut self) {
        let mut budget = self.free();
        while budget > 0 {
            let Some(waiter) = self.waiters.pop_front() else {
                break;
            };
            // A dropped receiver just consumes the wake; the next release
            // or drain wakes the following waiter.
            if waiter.send(()).is_ok() {
                budget -= 1;
            }
        }
    }
}

/// Point-in-time snapshot of the slot accounting, for tests and gauges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotSnapshot {
    pub capacity: u32,
    pub reserved: u32,
    pub live: u32,
    pub free: u32,
}

/// Busy-slot utilization integrals over virtual time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotUtilization {
    pub busy: Duration,
    pub capacity: Duration,
}

impl SlotUtilization {
    /// Fraction of available slot-time spent busy, in `[0, 1]`.
    #[must_use]
    pub fn fraction(&self) -> f64 {
        if self.capacity.is_zero() {
            return 0.0;
        }
        self.busy.as_secs_f64() / self.capacity.as_secs_f64()
    }
}

pub struct SlotView {
    state: Arc<Mutex<ViewState>>,
    allow_class_mismatch: bool,
}

impl Clone for SlotView {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            allow_class_mismatch: self.allow_class_mismatch,
        }
    }
}

impl SlotView {
    /// Seeds the view from `GetWorkerInfo` + `ListSlots` and spawns the
    /// periodic drain task. The caller owns the returned handle (abort it to
    /// stop maintenance; the view keeps working from reservations alone).
    pub async fn start<H>(
        hypervisor: H,
        config: SlotViewConfig,
    ) -> ClientResult<(Self, JoinHandle<()>)>
    where
        H: AsyncHypervisor + Clone + 'static,
    {
        let info = hypervisor.worker_info(GetWorkerInfoRequest).await?;
        let listed = hypervisor.list_slots(ListSlotsRequest).await?;
        let mut known = BTreeMap::new();
        for slot in listed.slots {
            known.insert(slot.slot_id, slot.state);
        }

        let state = Arc::new(Mutex::new(ViewState {
            capacity: info.slots_total,
            reserved: 0,
            known,
            waiters: VecDeque::new(),
            worker_class: info.class,
            busy_integral: Duration::ZERO,
            capacity_integral: Duration::ZERO,
            last_advance: Instant::now(),
        }));

        let view = Self {
            state: Arc::clone(&state),
            allow_class_mismatch: config.allow_class_mismatch,
        };
        let drain_interval = config.drain_interval;
        let task_state = state;
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(drain_interval).await;
                let events = match hypervisor.watch_slots(WatchSlotsRequest).await {
                    Ok(response) => response.events,
                    Err(_) => Vec::new(),
                };
                let capacity = hypervisor
                    .worker_info(GetWorkerInfoRequest)
                    .await
                    .ok()
                    .map(|info| info.slots_total);
                let mut view_state = task_state.lock().expect("slot view state poisoned");
                view_state.advance_integrals(Instant::now());
                for event in events {
                    view_state
                        .known
                        .insert(event.slot.slot_id, event.slot.state);
                }
                if let Some(capacity) = capacity {
                    view_state.capacity = capacity;
                }
                view_state.wake_free_waiters();
            }
        });

        Ok((view, handle))
    }

    /// Suspends until a class-compatible slot is likely free, then reserves
    /// it. Refuses immediately (fixed grep-able reason) on a determinism
    /// class mismatch unless `allow_class_mismatch` was configured.
    pub async fn acquire(
        &self,
        required_class: Option<&DeterminismClass>,
    ) -> ClientResult<SlotPermit> {
        if let Some(required) = required_class {
            let state = self.state.lock().expect("slot view state poisoned");
            if !self.allow_class_mismatch && *required != state.worker_class {
                return Err(ClientError::new(
                    ClientErrorKind::FailedPrecondition,
                    format!(
                        "{CLASS_MISMATCH_REASON}: job requires cpu_model={} vmm={}, worker offers cpu_model={} vmm={}",
                        required.cpu_model,
                        required.vmm_version,
                        state.worker_class.cpu_model,
                        state.worker_class.vmm_version,
                    ),
                ));
            }
        }

        loop {
            let receiver = {
                let mut state = self.state.lock().expect("slot view state poisoned");
                if state.free() > 0 {
                    state.advance_integrals(Instant::now());
                    state.reserved += 1;
                    return Ok(SlotPermit {
                        state: Arc::clone(&self.state),
                    });
                }
                let (sender, receiver) = oneshot::channel();
                state.waiters.push_back(sender);
                receiver
            };
            // A closed sender (view dropped mid-shutdown) just re-loops.
            let _ = receiver.await;
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> SlotSnapshot {
        let state = self.state.lock().expect("slot view state poisoned");
        SlotSnapshot {
            capacity: state.capacity,
            reserved: state.reserved,
            live: state.live(),
            free: state.free(),
        }
    }

    /// Busy/capacity integrals over virtual time, current up to now.
    #[must_use]
    pub fn utilization(&self) -> SlotUtilization {
        let mut state = self.state.lock().expect("slot view state poisoned");
        state.advance_integrals(Instant::now());
        SlotUtilization {
            busy: state.busy_integral,
            capacity: state.capacity_integral,
        }
    }

    #[must_use]
    pub fn worker_class(&self) -> DeterminismClass {
        self.state
            .lock()
            .expect("slot view state poisoned")
            .worker_class
            .clone()
    }
}

/// Reservation of one worker slot. Dropping releases the reservation and
/// wakes the next waiter.
pub struct SlotPermit {
    state: Arc<Mutex<ViewState>>,
}

impl std::fmt::Debug for SlotPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("SlotPermit").finish_non_exhaustive()
    }
}

impl Drop for SlotPermit {
    fn drop(&mut self) {
        let mut state = self.state.lock().expect("slot view state poisoned");
        state.advance_integrals(Instant::now());
        state.reserved = state.reserved.saturating_sub(1);
        state.wake_free_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::SyncAdapter;
    use orch_fakes::hypervisor::FakeHypervisor;

    async fn started(
        slots: u32,
        config: SlotViewConfig,
    ) -> (SlotView, JoinHandle<()>, SyncAdapter<FakeHypervisor>) {
        let adapter = SyncAdapter::new(FakeHypervisor::with_slots(slots));
        let (view, handle) = SlotView::start(adapter.clone(), config)
            .await
            .expect("slot view starts");
        (view, handle, adapter)
    }

    #[tokio::test(start_paused = true)]
    async fn acquire_reserves_and_release_wakes_fifo_waiters() {
        let (view, drain, _) = started(2, SlotViewConfig::default()).await;

        let first = view.acquire(None).await.expect("first slot");
        let second = view.acquire(None).await.expect("second slot");
        assert_eq!(view.snapshot().free, 0);

        let waiter_view = view.clone();
        let waiter = tokio::spawn(async move { waiter_view.acquire(None).await });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        drop(first);
        let third = waiter
            .await
            .expect("waiter task")
            .expect("woken waiter acquires");
        assert_eq!(view.snapshot().reserved, 2);

        drop(second);
        drop(third);
        assert_eq!(view.snapshot().free, 2);
        drain.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn class_mismatch_is_refused_with_grep_able_reason() {
        let (view, drain, _) = started(2, SlotViewConfig::default()).await;

        let matching = view.worker_class();
        assert!(view.acquire(Some(&matching)).await.is_ok());

        let mut wrong = view.worker_class();
        wrong.cpu_model = "other-cpu".to_owned();
        let error = view
            .acquire(Some(&wrong))
            .await
            .expect_err("class mismatch refused");
        assert_eq!(error.kind(), ClientErrorKind::FailedPrecondition);
        assert!(error.message().contains(CLASS_MISMATCH_REASON));
        drain.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn allow_class_mismatch_permits_dispatch() {
        let (view, drain, _) = started(
            1,
            SlotViewConfig {
                allow_class_mismatch: true,
                ..SlotViewConfig::default()
            },
        )
        .await;

        let mut wrong = view.worker_class();
        wrong.cpu_model = "other-cpu".to_owned();
        assert!(view.acquire(Some(&wrong)).await.is_ok());
        drain.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn drain_refreshes_capacity_for_shrink_and_grow() {
        let (view, drain, adapter) = started(8, SlotViewConfig::default()).await;
        assert_eq!(view.snapshot().capacity, 8);

        adapter.service().lock().await.set_slots_total(1);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(view.snapshot().capacity, 1);
        assert_eq!(view.snapshot().free, 1);

        let only = view.acquire(None).await.expect("single slot");
        let waiter_view = view.clone();
        let waiter = tokio::spawn(async move { waiter_view.acquire(None).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());

        adapter.service().lock().await.set_slots_total(8);
        tokio::time::sleep(Duration::from_millis(20)).await;
        let regrown = waiter
            .await
            .expect("waiter task")
            .expect("grow wakes waiter");
        drop(regrown);
        drop(only);
        drain.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn utilization_integrates_busy_slot_time() {
        let (view, drain, _) = started(4, SlotViewConfig::default()).await;

        let permits = [
            view.acquire(None).await.expect("slot"),
            view.acquire(None).await.expect("slot"),
        ];
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(permits);
        tokio::time::sleep(Duration::from_millis(100)).await;

        let utilization = view.utilization();
        assert_eq!(utilization.busy, Duration::from_millis(200));
        assert_eq!(utilization.capacity, Duration::from_millis(800));
        assert!((utilization.fraction() - 0.25).abs() < 1e-9);
        drain.abort();
    }
}
