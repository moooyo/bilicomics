use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, OnceLock, atomic::{AtomicBool, Ordering}},
    time::{Duration, Instant},
};

use bilicomics::ui::AppWindow;
use serde::Serialize;

/// A bounded notification token, independent from the number of completed jobs.
#[derive(Default)]
struct WakeGate {
    queued: AtomicBool,
    stopped: AtomicBool,
}

impl WakeGate {
    fn request(&self) -> bool {
        if self.stopped.load(Ordering::Acquire) || self.queued.swap(true, Ordering::AcqRel) {
            return false;
        }
        if self.stopped.load(Ordering::Acquire) {
            self.queued.store(false, Ordering::Release);
            return false;
        }
        true
    }

    fn begin_dispatch(&self) -> bool {
        self.queued.store(false, Ordering::Release);
        !self.stopped.load(Ordering::Acquire)
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.queued.store(false, Ordering::Release);
    }
}

/// Owns only a weak window reference; workers never retain the application UI.
#[derive(Clone)]
pub(crate) struct RuntimeWake {
    window: slint::Weak<AppWindow>,
    gate: Arc<WakeGate>,
}

impl RuntimeWake {
    pub(crate) fn new(window: slint::Weak<AppWindow>) -> Self {
        Self { window, gate: Arc::new(WakeGate::default()) }
    }

    pub(crate) fn request(&self) {
        if !self.gate.request() {
            return;
        }
        let gate = self.gate.clone();
        if self.window.upgrade_in_event_loop(move |window| {
            if gate.begin_dispatch() {
                window.invoke_runtime_wakeup();
            }
        }).is_err() {
            // A closed event loop is not a reason to retain a notification slot.
            self.gate.queued.store(false, Ordering::Release);
        }
    }

    pub(crate) fn stop(&self) {
        self.gate.stop();
    }
}

/// Continue finite startup work on a later timer turn so rendering can run.
/// One pending continuation is shared by maintenance and worker notifications.
#[derive(Clone)]
pub(crate) struct StartupContinuation {
    scheduled: Rc<Cell<bool>>,
    wake: RuntimeWake,
}

impl StartupContinuation {
    pub(crate) fn new(wake: RuntimeWake) -> Self {
        Self { scheduled: Rc::new(Cell::new(false)), wake }
    }

    pub(crate) fn schedule(&self) {
        if self.scheduled.replace(true) {
            return;
        }
        let scheduled = self.scheduled.clone();
        let wake = self.wake.clone();
        slint::Timer::single_shot(Duration::from_millis(1), move || {
            scheduled.set(false);
            wake.request();
        });
    }
}

struct TraceClock {
    started: Instant,
    monotonic_start_ns: Option<u64>,
}

static TRACE_CLOCK: OnceLock<TraceClock> = OnceLock::new();

#[derive(Clone, Copy, Serialize)]
pub(crate) struct TraceMoment {
    pub(crate) monotonic_ns: Option<u64>,
    pub(crate) elapsed_ms: f64,
}

pub(crate) fn initialize_trace_clock() {
    let _ = TRACE_CLOCK.get_or_init(|| TraceClock {
        started: Instant::now(), monotonic_start_ns: monotonic_ns(),
    });
}

pub(crate) fn process_entry_moment() -> TraceMoment {
    initialize_trace_clock();
    TraceMoment {
        monotonic_ns: TRACE_CLOCK.get().and_then(|clock| clock.monotonic_start_ns),
        elapsed_ms: 0.0,
    }
}

pub(crate) fn trace_moment() -> TraceMoment {
    initialize_trace_clock();
    TraceMoment {
        monotonic_ns: monotonic_ns(),
        elapsed_ms: TRACE_CLOCK.get().map_or(0.0, |clock| clock.started.elapsed().as_secs_f64() * 1000.0),
    }
}

#[cfg(target_os = "linux")]
fn monotonic_ns() -> Option<u64> {
    let mut stamp = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // CLOCK_MONOTONIC shares the origin used by host-side timing probes.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut stamp) } != 0 {
        return None;
    }
    u64::try_from(stamp.tv_sec).ok()?.checked_mul(1_000_000_000)?
        .checked_add(u64::try_from(stamp.tv_nsec).ok()?)
}

#[cfg(not(target_os = "linux"))]
fn monotonic_ns() -> Option<u64> { None }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_concurrent_burst_owns_one_notification_until_dispatch() {
        let gate = Arc::new(WakeGate::default());
        let admitted = std::thread::scope(|scope| {
            let threads = (0..32).map(|_| {
                let gate = gate.clone();
                scope.spawn(move || usize::from(gate.request()))
            }).collect::<Vec<_>>();
            threads.into_iter().map(|thread| thread.join().unwrap()).sum::<usize>()
        });
        assert_eq!(admitted, 1);
        assert!(gate.begin_dispatch());
        assert!(gate.request());
        assert!(!gate.request());
    }

    #[test]
    fn shutdown_rejects_pending_and_future_wakeups() {
        let gate = WakeGate::default();
        assert!(gate.request());
        gate.stop();
        assert!(!gate.begin_dispatch());
        assert!(!gate.request());
        assert!(!gate.queued.load(Ordering::Acquire));
    }

    #[test]
    fn dispatch_releases_capacity_before_accepting_a_following_result() {
        let gate = WakeGate::default();
        assert!(gate.request());
        assert!(gate.begin_dispatch());
        assert!(gate.request());
        assert!(!gate.request());
    }
}
