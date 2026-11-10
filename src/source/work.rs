//! Process-wide admission for actual blocking computation.
//!
//! The memory amount is a conservative planned-work reservation, not an RSS cap.
//! Callers must acquire at a worker boundary and must not block on another
//! source job while inside an admitted closure. Pure validators do not acquire.

use std::{
    cell::Cell,
    sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError, Weak, atomic::{AtomicBool, Ordering}},
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(crate) const COMPUTE_SLOTS: usize = 2;
pub(crate) const PLANNED_MEMORY_BYTES: u64 = 384 * 1024 * 1024;
pub(crate) const HEAVY_WORK_BYTES: u64 = 192 * 1024 * 1024;
pub(crate) const COVER_WORK_BYTES: u64 = 80 * 1024 * 1024;
pub(crate) const METADATA_WORK_BYTES: u64 = 4 * 1024 * 1024;
const MEMORY_UNIT_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum WorkError {
    #[error("blocking work was cancelled")]
    Cancelled,
    #[error("blocking work exceeds the process memory budget")]
    BudgetExceeded,
    #[error("the blocking work budget is unavailable or nested")]
    BudgetUnavailable,
    #[error("the blocking worker failed")]
    WorkerFailed,
}

#[derive(Clone)]
pub(crate) struct WorkCancellation(Arc<AtomicBool>);

impl WorkCancellation {
    pub(crate) fn is_cancelled(&self) -> bool { self.0.load(Ordering::Acquire) }
    pub(crate) fn check(&self) -> Result<(), WorkError> {
        if self.is_cancelled() { Err(WorkError::Cancelled) } else { Ok(()) }
    }
}

struct CancelOnDrop { cancellation: WorkCancellation, armed: bool }

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed { self.cancellation.0.store(true, Ordering::Release); }
    }
}

struct ComputeBudget {
    cpu: Arc<Semaphore>,
    cpu_slots: usize,
    memory: Arc<Semaphore>,
    memory_units: u32,
    admission: Arc<Mutex<AdmissionState>>,
}

#[derive(Default)]
struct AdmissionState {
    regular_waiters: usize,
    speculative_generation: u64,
    speculative: Option<(u64, Weak<AtomicBool>)>,
}

impl AdmissionState {
    fn cancel_speculative(&self) {
        if let Some(cancelled) = self.speculative.as_ref().and_then(|(_, flag)| flag.upgrade()) {
            cancelled.store(true, Ordering::Release);
        }
    }
}

fn lock_admission(admission: &Mutex<AdmissionState>) -> MutexGuard<'_, AdmissionState> {
    admission.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct RegularWaiter(Arc<Mutex<AdmissionState>>);

impl Drop for RegularWaiter {
    fn drop(&mut self) {
        let mut state = lock_admission(&self.0);
        state.regular_waiters -= 1;
    }
}

struct SpeculativeSlot {
    admission: Arc<Mutex<AdmissionState>>,
    generation: u64,
}

impl Drop for SpeculativeSlot {
    fn drop(&mut self) {
        let mut state = lock_admission(&self.admission);
        if state.speculative.as_ref().is_some_and(|(generation, _)| *generation == self.generation) {
            state.speculative = None;
        }
    }
}

/// A speculative reservation belongs to its actual blocking worker, including
/// after the UI marker or the future awaiting that worker has been discarded.
pub(crate) struct SpeculativeLease {
    // Return the memory and CPU permits before admitting another prediction.
    _compute: ComputeLease,
    _slot: SpeculativeSlot,
    cancellation: WorkCancellation,
}

impl ComputeBudget {
    fn new(cpu_slots: usize, memory_bytes: u64) -> Self {
        let memory_units = u32::try_from(memory_bytes / MEMORY_UNIT_BYTES)
            .expect("the configured memory budget must fit semaphore accounting");
        Self { cpu: Arc::new(Semaphore::new(cpu_slots)), cpu_slots,
            memory: Arc::new(Semaphore::new(memory_units as usize)), memory_units,
            admission: Arc::new(Mutex::new(AdmissionState::default())) }
    }

    fn regular_waiter(&self) -> Result<RegularWaiter, WorkError> {
        let mut state = lock_admission(&self.admission);
        state.regular_waiters = state.regular_waiters.checked_add(1)
            .ok_or(WorkError::BudgetUnavailable)?;
        state.cancel_speculative();
        Ok(RegularWaiter(self.admission.clone()))
    }

    fn try_speculative(&self, memory_bytes: u64, cancelled: Arc<AtomicBool>) -> Option<SpeculativeLease> {
        if IN_BLOCKING_WORK.with(Cell::get) || cancelled.load(Ordering::Acquire) { return None; }
        let units = u32::try_from(memory_bytes.div_ceil(MEMORY_UNIT_BYTES).max(1)).ok()?;
        if units > self.memory_units { return None; }
        let mut state = match self.admission.try_lock() {
            Ok(state) => state,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        if state.regular_waiters != 0 || state.speculative.is_some()
            || self.cpu.available_permits() != self.cpu_slots
            || cancelled.load(Ordering::Acquire) { return None; }
        let generation = state.speculative_generation.checked_add(1)?;
        // These are nonblocking attempts, so no CPU slot can wait for memory.
        // Ordinary callers retain their existing FIFO CPU-then-memory order.
        let memory = self.memory.clone().try_acquire_many_owned(units).ok()?;
        let cpu = self.cpu.clone().try_acquire_owned().ok()?;
        state.speculative_generation = generation;
        state.speculative = Some((generation, Arc::downgrade(&cancelled)));
        Some(SpeculativeLease { _compute: ComputeLease { _memory: memory, _cpu: cpu },
            _slot: SpeculativeSlot { admission: self.admission.clone(), generation },
            cancellation: WorkCancellation(cancelled) })
    }

    async fn acquire(&self, memory_bytes: u64, cancellation: &WorkCancellation) -> Result<ComputeLease, WorkError> {
        let units = u32::try_from(memory_bytes.div_ceil(MEMORY_UNIT_BYTES).max(1))
            .map_err(|_| WorkError::BudgetExceeded)?;
        if units > self.memory_units { return Err(WorkError::BudgetExceeded); }
        cancellation.check()?;
        let _waiter = self.regular_waiter()?;
        // Ordinary callers take resources in this order. A CPU permit is never
        // released while a running worker still owns its memory reservation.
        let cpu = self.cpu.clone().acquire_owned().await.map_err(|_| WorkError::BudgetUnavailable)?;
        cancellation.check()?;
        let memory = self.memory.clone().acquire_many_owned(units).await
            .map_err(|_| WorkError::BudgetUnavailable)?;
        cancellation.check()?;
        Ok(ComputeLease { _memory: memory, _cpu: cpu })
    }
}

struct ComputeLease {
    // Drop memory before CPU so a newly admitted worker can use the reservation.
    _memory: OwnedSemaphorePermit,
    _cpu: OwnedSemaphorePermit,
}

fn shared_budget() -> &'static ComputeBudget {
    static BUDGET: OnceLock<ComputeBudget> = OnceLock::new();
    BUDGET.get_or_init(|| ComputeBudget::new(COMPUTE_SLOTS, PLANNED_MEMORY_BYTES))
}

#[cfg(test)]
pub(crate) fn process_test_lock() -> &'static tokio::sync::Mutex<()> {
    // Tests that deliberately occupy every process slot must not wait on
    // another test that also deliberately occupies every process slot.
    static SERIAL: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    SERIAL.get_or_init(|| tokio::sync::Mutex::new(()))
}

thread_local! { static IN_BLOCKING_WORK: Cell<bool> = const { Cell::new(false) }; }

struct BlockingScope;

impl BlockingScope {
    fn enter() -> Self {
        IN_BLOCKING_WORK.with(|active| active.set(true));
        Self
    }
}

impl Drop for BlockingScope {
    fn drop(&mut self) { IN_BLOCKING_WORK.with(|active| active.set(false)); }
}

/// Admit before queueing work, then retain both permits inside the real worker.
/// Dropping the awaiting future cancels queued or obsolete work at its next check.
pub(crate) async fn run_blocking<T, F>(memory_cost: u64, job: F) -> Result<T, WorkError>
where
    T: Send + 'static,
    F: FnOnce(&WorkCancellation) -> T + Send + 'static,
{
    run_with_budget(shared_budget(), memory_cost, job).await
}

/// Observe admission separately from execution without changing resource ownership.
pub(crate) async fn run_blocking_traced<T, F>(memory_cost: u64,
    trace: Option<crate::performance::Operation>, job: F) -> Result<T, WorkError>
where
    T: Send + 'static,
    F: FnOnce(&WorkCancellation) -> T + Send + 'static,
{
    run_with_budget_traced(shared_budget(), memory_cost, trace, job).await
}

/// Reserve a prediction synchronously, without entering either budget queue.
/// All compute slots must be idle and no ordinary budget waiter may be present.
/// Failed admission creates no waiter and retains no partial reservation.
pub(crate) fn try_speculative(memory_cost: u64, cancelled: Arc<AtomicBool>) -> Option<SpeculativeLease> {
    shared_budget().try_speculative(memory_cost, cancelled)
}

/// Keep the speculative lease inside actual blocking work. The callback also
/// runs on cancellation so it can return its result-delivery envelope; it must
/// check the shared cancellation before starting or retaining expensive work.
pub(crate) async fn run_speculative_traced<T, F>(lease: SpeculativeLease,
    trace: Option<crate::performance::Operation>, job: F) -> Result<T, WorkError>
where
    T: Send + 'static,
    F: FnOnce(&WorkCancellation) -> T + Send + 'static,
{
    let cancellation = lease.cancellation.clone();
    let mut cancel_on_drop = CancelOnDrop { cancellation: cancellation.clone(), armed: true };
    let result = tokio::task::spawn_blocking(move || {
        let _lease = lease;
        let _scope = BlockingScope::enter();
        if let Some(trace) = trace { trace.mark("speculative_worker_begin"); }
        let measured = trace.map(|_| crate::performance::Span::start());
        let result = job(&cancellation);
        if let (Some(trace), Some(measured)) = (trace, measured) {
            trace.record("speculative_worker_end", &measured.finish());
            if cancellation.is_cancelled() { trace.mark("speculative_worker_cancelled"); }
        }
        result
    }).await.map_err(|_| WorkError::WorkerFailed)?;
    cancel_on_drop.armed = false;
    Ok(result)
}

async fn run_with_budget<T, F>(budget: &ComputeBudget, memory_cost: u64, job: F) -> Result<T, WorkError>
where
    T: Send + 'static,
    F: FnOnce(&WorkCancellation) -> T + Send + 'static,
{
    run_with_budget_traced(budget, memory_cost, None, job).await
}

async fn run_with_budget_traced<T, F>(budget: &ComputeBudget, memory_cost: u64,
    trace: Option<crate::performance::Operation>, job: F) -> Result<T, WorkError>
where
    T: Send + 'static,
    F: FnOnce(&WorkCancellation) -> T + Send + 'static,
{
    if IN_BLOCKING_WORK.with(Cell::get) { return Err(WorkError::BudgetUnavailable); }
    let cancellation = WorkCancellation(Arc::new(AtomicBool::new(false)));
    let mut cancel_on_drop = CancelOnDrop { cancellation: cancellation.clone(), armed: true };
    if let Some(trace) = trace { trace.mark("compute_wait_begin"); }
    let lease = budget.acquire(memory_cost, &cancellation).await?;
    if let Some(trace) = trace {
        trace.record("compute_admitted", &serde_json::json!({"reserved_bytes": memory_cost}));
    }
    let result = tokio::task::spawn_blocking(move || {
        let _lease = lease;
        cancellation.check()?;
        let _scope = BlockingScope::enter();
        if let Some(trace) = trace { trace.mark("worker_begin"); }
        let measured = trace.map(|_| crate::performance::Span::start());
        let result = job(&cancellation);
        if let (Some(trace), Some(measured)) = (trace, measured) {
            trace.record("worker_end", &measured.finish());
        }
        cancellation.check()?;
        Ok(result)
    }).await.map_err(|_| WorkError::WorkerFailed)?;
    cancel_on_drop.armed = false;
    result
}

pub(super) fn source_error(error: WorkError, worker_code: &'static str) -> super::SourceError {
    match error {
        WorkError::Cancelled => super::SourceError::new(super::ErrorKind::Cancelled, "source_work_cancelled"),
        WorkError::BudgetExceeded => super::SourceError::new(super::ErrorKind::Capability, "source_work_memory_limit"),
        WorkError::BudgetUnavailable => super::SourceError::new(super::ErrorKind::Capability, "source_work_budget_unavailable"),
        WorkError::WorkerFailed => super::SourceError::capability(worker_code),
    }
}

#[cfg(test)]
#[path = "work_tests.rs"]
mod tests;
