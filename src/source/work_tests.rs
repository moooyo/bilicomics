use super::*;

use std::{sync::{Condvar, Mutex, atomic::AtomicUsize}, time::Duration};

const MIB: u64 = 1024 * 1024;

struct Gate {
    entered: Semaphore,
    released: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self { entered: Semaphore::new(0), released: Mutex::new(false), changed: Condvar::new() })
    }
    fn wait(&self) {
        self.entered.add_permits(1);
        let mut released = self.released.lock().unwrap_or_else(|error| error.into_inner());
        while !*released { released = self.changed.wait(released).unwrap_or_else(|error| error.into_inner()); }
    }
    fn release(&self) {
        *self.released.lock().unwrap_or_else(|error| error.into_inner()) = true;
        self.changed.notify_all();
    }
    async fn wait_for(&self, count: u32) {
        tokio::time::timeout(Duration::from_secs(8), self.entered.acquire_many(count)).await
            .expect("The bounded synthetic worker must enter").unwrap().forget();
    }
}

struct ReleaseOnDrop(Arc<Gate>);
impl Drop for ReleaseOnDrop { fn drop(&mut self) { self.0.release(); } }

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(8), async {
        while !predicate() { tokio::task::yield_now().await; }
    }).await.expect("The synthetic budget did not reach its expected state");
}

#[tokio::test]
async fn cancelling_the_awaiter_keeps_both_reservations_in_the_running_closure() {
    let budget = Arc::new(ComputeBudget::new(1, 4 * MIB));
    let gate = Gate::new();
    let _release = ReleaseOnDrop(gate.clone());
    let saw_cancel = Arc::new(AtomicBool::new(false));
    let task_budget = budget.clone(); let task_gate = gate.clone(); let observed = saw_cancel.clone();
    let first = tokio::spawn(async move {
        run_with_budget(&task_budget, 4 * MIB, move |cancel| {
            task_gate.wait();
            observed.store(cancel.is_cancelled(), Ordering::Release);
        }).await
    });
    gate.wait_for(1).await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert_eq!(budget.cpu.available_permits(), 0);
    assert_eq!(budget.memory.available_permits(), 0);
    let next_budget = budget.clone();
    let second = tokio::spawn(async move { run_with_budget(&next_budget, 4 * MIB, |_| 23).await });
    gate.release();
    assert_eq!(tokio::time::timeout(Duration::from_secs(8), second).await.unwrap().unwrap().unwrap(), 23);
    assert!(saw_cancel.load(Ordering::Acquire));
    assert_eq!(budget.cpu.available_permits(), 1);
    assert_eq!(budget.memory.available_permits(), 4);
}

#[tokio::test]
async fn cancelling_a_budget_waiter_never_queues_its_work_or_leaks_a_permit() {
    let budget = Arc::new(ComputeBudget::new(1, 2 * MIB));
    let gate = Gate::new(); let _release = ReleaseOnDrop(gate.clone());
    let first_budget = budget.clone(); let first_gate = gate.clone();
    let first = tokio::spawn(async move { run_with_budget(&first_budget, 2 * MIB, move |_| first_gate.wait()).await });
    gate.wait_for(1).await;
    let called = Arc::new(AtomicBool::new(false));
    let next_budget = budget.clone(); let next_called = called.clone();
    let waiting = tokio::spawn(async move {
        run_with_budget(&next_budget, 2 * MIB, move |_| next_called.store(true, Ordering::Release)).await
    });
    tokio::task::yield_now().await;
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    gate.release(); first.await.unwrap().unwrap();
    run_with_budget(&budget, 2 * MIB, |_| ()).await.unwrap();
    assert!(!called.load(Ordering::Acquire));
    assert_eq!(budget.cpu.available_permits(), 1);
    assert_eq!(budget.memory.available_permits(), 2);
}

#[tokio::test]
async fn planned_memory_can_limit_parallel_work_below_the_cpu_slot_count() {
    let budget = Arc::new(ComputeBudget::new(2, 3 * MIB));
    let gate = Gate::new(); let _release = ReleaseOnDrop(gate.clone());
    let first_budget = budget.clone(); let first_gate = gate.clone();
    let first = tokio::spawn(async move { run_with_budget(&first_budget, 2 * MIB, move |_| first_gate.wait()).await });
    gate.wait_for(1).await;
    let called = Arc::new(AtomicBool::new(false));
    let waiting_called = called.clone();
    let mut waiting = Box::pin(run_with_budget(&budget, 2 * MIB,
        move |_| waiting_called.store(true, Ordering::Release)));
    std::future::poll_fn(|context| {
        assert!(std::future::Future::poll(waiting.as_mut(), context).is_pending());
        std::task::Poll::Ready(())
    }).await;
    assert!(!called.load(Ordering::Acquire));
    // Fair multi-permit waiters may reserve a partial allocation internally.
    // Cancelling that actual future must return both resources immediately.
    drop(waiting);
    assert_eq!(budget.cpu.available_permits(), 1);
    assert_eq!(budget.memory.available_permits(), 1);
    run_with_budget(&budget, MIB, |_| ()).await.unwrap();
    gate.release(); first.await.unwrap().unwrap();
    assert_eq!(budget.memory.available_permits(), 3);
}

#[test]
fn cancelled_work_already_in_the_blocking_queue_skips_its_allocation_callback() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1)
        .max_blocking_threads(1).enable_all().build().unwrap();
    let gate = Gate::new(); let _release = ReleaseOnDrop(gate.clone());
    let occupied_gate = gate.clone();
    let occupied = runtime.spawn_blocking(move || occupied_gate.wait());
    runtime.block_on(gate.wait_for(1));
    let budget = Arc::new(ComputeBudget::new(1, 4 * MIB));
    let called = Arc::new(AtomicBool::new(false));
    let task_budget = budget.clone(); let task_called = called.clone();
    let queued = runtime.spawn(async move {
        run_with_budget(&task_budget, 4 * MIB, move |_| task_called.store(true, Ordering::Release)).await
    });
    runtime.block_on(wait_until(|| budget.cpu.available_permits() == 0));
    queued.abort(); assert!(runtime.block_on(queued).unwrap_err().is_cancelled());
    assert_eq!(budget.memory.available_permits(), 0);
    gate.release(); runtime.block_on(occupied).unwrap();
    runtime.block_on(wait_until(|| budget.cpu.available_permits() == 1));
    assert!(!called.load(Ordering::Acquire));
    assert_eq!(budget.memory.available_permits(), 4);
}

#[tokio::test]
async fn an_oversized_job_is_rejected_without_calling_the_worker() {
    let budget = ComputeBudget::new(2, 3 * MIB);
    let called = Arc::new(AtomicBool::new(false)); let observed = called.clone();
    assert_eq!(run_with_budget(&budget, 4 * MIB, move |_| observed.store(true, Ordering::Release)).await,
        Err(WorkError::BudgetExceeded));
    assert!(!called.load(Ordering::Acquire));
    assert_eq!(budget.cpu.available_permits(), 2);
}

#[tokio::test]
async fn accidentally_blocking_on_nested_work_returns_an_error_instead_of_deadlocking() {
    let budget = Arc::new(ComputeBudget::new(1, 4 * MIB));
    let nested_budget = budget.clone();
    let handle = tokio::runtime::Handle::current();
    let result = run_with_budget(&budget, 4 * MIB, move |_| {
        handle.block_on(run_with_budget(&nested_budget, MIB, |_| ()))
    }).await.unwrap();
    assert_eq!(result, Err(WorkError::BudgetUnavailable));
}

#[tokio::test]
async fn independent_producers_use_the_same_process_gate() {
    let _serial = process_test_lock().lock().await;
    let gate = Gate::new(); let _release = ReleaseOnDrop(gate.clone());
    let active = Arc::new(AtomicUsize::new(0)); let maximum = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for _ in 0..6 {
        let gate = gate.clone(); let active = active.clone(); let maximum = maximum.clone();
        tasks.push(tokio::spawn(async move {
            run_blocking(METADATA_WORK_BYTES, move |_| {
                let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(count, Ordering::SeqCst);
                gate.wait(); active.fetch_sub(1, Ordering::SeqCst);
            }).await
        }));
    }
    gate.wait_for(COMPUTE_SLOTS as u32).await;
    assert!(maximum.load(Ordering::SeqCst) <= COMPUTE_SLOTS);
    gate.release();
    for task in tasks { task.await.unwrap().unwrap(); }
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn speculative_admission_cannot_pass_a_regular_waiter_before_it_resumes() {
    let budget = ComputeBudget::new(2, 4 * MIB);
    let registration = budget.regular_waiter().unwrap();
    assert_eq!(budget.cpu.available_permits(), 2);
    assert_eq!(budget.memory.available_permits(), 4);
    assert!(budget.try_speculative(MIB, Arc::new(AtomicBool::new(false))).is_none());
    drop(registration);
    let held = budget.memory.clone().acquire_many_owned(4).await.unwrap();
    let called = Arc::new(AtomicBool::new(false));
    let callback_called = called.clone();
    let mut regular = Box::pin(run_with_budget(&budget, 2 * MIB,
        move |_| callback_called.store(true, Ordering::Release)));
    std::future::poll_fn(|context| {
        assert!(std::future::Future::poll(regular.as_mut(), context).is_pending());
        std::task::Poll::Ready(())
    }).await;
    assert_eq!(lock_admission(&budget.admission).regular_waiters, 1);
    drop(held);
    // The semaphore has granted memory, but the foreground future has not
    // resumed. Spare resources must not let a prediction pass its registration.
    assert_eq!(budget.cpu.available_permits(), 1);
    assert_eq!(budget.memory.available_permits(), 2);
    assert!(budget.try_speculative(MIB, Arc::new(AtomicBool::new(false))).is_none());
    assert_eq!(budget.cpu.available_permits(), 1);
    assert_eq!(budget.memory.available_permits(), 2);
    drop(regular);
    assert!(!called.load(Ordering::Acquire));
    assert_eq!(lock_admission(&budget.admission).regular_waiters, 0);
    assert_eq!(budget.cpu.available_permits(), 2);
    assert_eq!(budget.memory.available_permits(), 4);
    assert!(budget.try_speculative(MIB, Arc::new(AtomicBool::new(false))).is_some());
}

#[tokio::test]
async fn speculative_admission_requires_every_cpu_slot_to_be_idle() {
    let budget = Arc::new(ComputeBudget::new(2, 4 * MIB));
    let gate = Gate::new(); let _release = ReleaseOnDrop(gate.clone());
    let worker_budget = budget.clone(); let worker_gate = gate.clone();
    let regular = tokio::spawn(async move {
        run_with_budget(&worker_budget, MIB, move |_| worker_gate.wait()).await
    });
    gate.wait_for(1).await;
    assert_eq!(lock_admission(&budget.admission).regular_waiters, 0);
    assert_eq!(budget.cpu.available_permits(), 1);
    assert_eq!(budget.memory.available_permits(), 3);
    assert!(budget.try_speculative(MIB, Arc::new(AtomicBool::new(false))).is_none());
    gate.release();
    regular.await.unwrap().unwrap();
    assert!(budget.try_speculative(MIB, Arc::new(AtomicBool::new(false))).is_some());
}

#[tokio::test]
async fn foreground_work_uses_the_other_slot_and_cancels_the_original_speculative_flag() {
    let budget = ComputeBudget::new(2, 4 * MIB);
    let cancelled = Arc::new(AtomicBool::new(false));
    let lease = budget.try_speculative(2 * MIB, cancelled.clone()).unwrap();
    let gate = Gate::new(); let _release = ReleaseOnDrop(gate.clone());
    let worker_gate = gate.clone();
    let speculative = tokio::spawn(run_speculative_traced(lease, None, move |cancel| {
        worker_gate.wait();
        cancel.is_cancelled()
    }));
    gate.wait_for(1).await;
    let result = tokio::time::timeout(Duration::from_secs(8),
        run_with_budget(&budget, 2 * MIB, |_| 41)).await.unwrap().unwrap();
    assert_eq!(result, 41);
    assert!(cancelled.load(Ordering::Acquire));
    assert_eq!(budget.cpu.available_permits(), 1);
    assert_eq!(budget.memory.available_permits(), 2);
    assert!(lock_admission(&budget.admission).speculative.is_some());
    gate.release();
    assert!(speculative.await.unwrap().unwrap());
    assert!(lock_admission(&budget.admission).speculative.is_none());
}

#[tokio::test]
async fn dropping_a_speculative_future_or_marker_keeps_the_actual_worker_exclusive() {
    let budget = ComputeBudget::new(2, 4 * MIB);
    let mut marker = Some(Arc::new(AtomicBool::new(false)));
    let cancelled = marker.as_ref().unwrap().clone();
    let lease = budget.try_speculative(2 * MIB, cancelled.clone()).unwrap();
    let gate = Gate::new(); let _release = ReleaseOnDrop(gate.clone());
    let worker_gate = gate.clone();
    let (exited, did_exit) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(run_speculative_traced(lease, None, move |cancel| {
        worker_gate.wait();
        let _ = exited.send(cancel.is_cancelled());
    }));
    gate.wait_for(1).await;
    drop(marker.take());
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    assert!(cancelled.load(Ordering::Acquire));
    assert_eq!(budget.cpu.available_permits(), 1);
    assert_eq!(budget.memory.available_permits(), 2);
    for _ in 0..4 {
        assert!(budget.try_speculative(MIB, Arc::new(AtomicBool::new(false))).is_none());
    }
    assert_eq!(budget.cpu.available_permits(), 1);
    assert_eq!(budget.memory.available_permits(), 2);
    gate.release();
    assert!(did_exit.await.unwrap());
    wait_until(|| lock_admission(&budget.admission).speculative.is_none()).await;
    assert_eq!(budget.cpu.available_permits(), 2);
    assert_eq!(budget.memory.available_permits(), 4);
    assert!(budget.try_speculative(MIB, Arc::new(AtomicBool::new(false))).is_some());
}

#[tokio::test]
async fn speculative_memory_failure_never_retains_a_cpu_slot_or_partial_reservation() {
    let budget = ComputeBudget::new(2, 4 * MIB);
    let held_memory = budget.memory.clone().acquire_many_owned(3).await.unwrap();
    for _ in 0..4 {
        assert!(budget.try_speculative(2 * MIB, Arc::new(AtomicBool::new(false))).is_none());
        assert_eq!(budget.cpu.available_permits(), 2);
        assert_eq!(budget.memory.available_permits(), 1);
        assert!(lock_admission(&budget.admission).speculative.is_none());
        assert_eq!(lock_admission(&budget.admission).regular_waiters, 0);
    }
    drop(held_memory);
    let held_cpu = budget.cpu.clone().acquire_many_owned(2).await.unwrap();
    assert!(budget.try_speculative(2 * MIB, Arc::new(AtomicBool::new(false))).is_none());
    assert_eq!(budget.memory.available_permits(), 4);
    assert!(lock_admission(&budget.admission).speculative.is_none());
    drop(held_cpu);
    assert!(budget.try_speculative(2 * MIB, Arc::new(AtomicBool::new(false))).is_some());
}

#[tokio::test]
async fn speculative_panic_and_error_release_the_actual_worker_reservations() {
    let budget = ComputeBudget::new(2, 4 * MIB);
    let lease = budget.try_speculative(2 * MIB, Arc::new(AtomicBool::new(false))).unwrap();
    let result = run_speculative_traced(lease, None, |_| -> Result<(), &'static str> {
        Err("Controlled speculative failure")
    }).await.unwrap();
    assert!(result.is_err());
    assert!(lock_admission(&budget.admission).speculative.is_none());
    assert_eq!(budget.cpu.available_permits(), 2);
    assert_eq!(budget.memory.available_permits(), 4);
    let lease = budget.try_speculative(2 * MIB, Arc::new(AtomicBool::new(false))).unwrap();
    let result = run_speculative_traced(lease, None, |_| -> () {
        panic!("Controlled speculative panic");
    }).await;
    assert_eq!(result, Err(WorkError::WorkerFailed));
    assert!(lock_admission(&budget.admission).speculative.is_none());
    assert_eq!(budget.cpu.available_permits(), 2);
    assert_eq!(budget.memory.available_permits(), 4);
    assert_eq!(run_with_budget(&budget, MIB, |_| 13).await.unwrap(), 13);
}

#[test]
fn speculative_admission_rejects_a_busy_metadata_lock_and_cancelled_work_immediately() {
    let budget = ComputeBudget::new(2, 4 * MIB);
    let state = lock_admission(&budget.admission);
    assert!(budget.try_speculative(MIB, Arc::new(AtomicBool::new(false))).is_none());
    assert_eq!(budget.cpu.available_permits(), 2);
    assert_eq!(budget.memory.available_permits(), 4);
    drop(state);
    assert!(budget.try_speculative(MIB, Arc::new(AtomicBool::new(true))).is_none());
    assert!(budget.try_speculative(5 * MIB, Arc::new(AtomicBool::new(false))).is_none());
    assert!(lock_admission(&budget.admission).speculative.is_none());
    assert_eq!(lock_admission(&budget.admission).regular_waiters, 0);
}
