//! Process-wide admission for actual blocking computation.
//!
//! The memory amount is a conservative planned-work reservation, not an RSS cap.
//! Callers must acquire at a worker boundary and must not block on another
//! source job while inside an admitted closure. Pure validators do not acquire.

use std::{
    cell::Cell,
    sync::{Arc, OnceLock, atomic::{AtomicBool, Ordering}},
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
    memory: Arc<Semaphore>,
    memory_units: u32,
}

impl ComputeBudget {
    fn new(cpu_slots: usize, memory_bytes: u64) -> Self {
        let memory_units = u32::try_from(memory_bytes / MEMORY_UNIT_BYTES)
            .expect("the configured memory budget must fit semaphore accounting");
        Self { cpu: Arc::new(Semaphore::new(cpu_slots)),
            memory: Arc::new(Semaphore::new(memory_units as usize)), memory_units }
    }

    async fn acquire(&self, memory_bytes: u64, cancellation: &WorkCancellation) -> Result<ComputeLease, WorkError> {
        let units = u32::try_from(memory_bytes.div_ceil(MEMORY_UNIT_BYTES).max(1))
            .map_err(|_| WorkError::BudgetExceeded)?;
        if units > self.memory_units { return Err(WorkError::BudgetExceeded); }
        cancellation.check()?;
        // All callers take resources in this order. A CPU permit is never
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

async fn run_with_budget<T, F>(budget: &ComputeBudget, memory_cost: u64, job: F) -> Result<T, WorkError>
where
    T: Send + 'static,
    F: FnOnce(&WorkCancellation) -> T + Send + 'static,
{
    if IN_BLOCKING_WORK.with(Cell::get) { return Err(WorkError::BudgetUnavailable); }
    let cancellation = WorkCancellation(Arc::new(AtomicBool::new(false)));
    let mut cancel_on_drop = CancelOnDrop { cancellation: cancellation.clone(), armed: true };
    let lease = budget.acquire(memory_cost, &cancellation).await?;
    let result = tokio::task::spawn_blocking(move || {
        let _lease = lease;
        cancellation.check()?;
        let _scope = BlockingScope::enter();
        let result = job(&cancellation);
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
