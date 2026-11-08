//! Lets background threads wake up the event loop on the UI thread.
//!
//! A pipe is shared between the loop (which waits on the read end) and any
//! background threads (which write a byte to the write end to nudge the loop).
//! Writing wakes the loop so queued closures can run on the UI thread.
//!
//! A pipe is used instead of `eventfd` so the crate still builds on macOS,
//! where `libc::eventfd` isn't available.

use std::collections::VecDeque;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use slint::EventLoopError;
use slint::platform::EventLoopProxy;

pub(crate) type Closure = Box<dyn FnOnce() + Send>;
pub(crate) const MAX_QUEUED_CLOSURES: usize = 256;
pub(crate) type Queue = Arc<Mutex<ClosureQueue>>;

#[derive(Clone, Copy)]
enum StopReason {
    Requested,
    CapacityExceeded,
    Poisoned,
}

pub(crate) struct ClosureQueue {
    pending: VecDeque<Closure>,
    stopped: Option<StopReason>,
}

pub(crate) fn new_queue() -> Queue {
    Arc::new(Mutex::new(ClosureQueue {
        pending: VecDeque::with_capacity(MAX_QUEUED_CLOSURES),
        stopped: None,
    }))
}

fn lock_queue<'a>(queue: &'a Queue, quit: &AtomicBool) -> MutexGuard<'a, ClosureQueue> {
    match queue.lock() {
        Ok(state) => state,
        Err(poisoned) => {
            // Shutdown must still release captured resources after poisoning.
            let mut state = poisoned.into_inner();
            state.stopped.get_or_insert(StopReason::Poisoned);
            quit.store(true, Ordering::SeqCst);
            state
        }
    }
}

fn close_locked(
    state: &mut ClosureQueue,
    quit: &AtomicBool,
    reason: StopReason,
) -> VecDeque<Closure> {
    state.stopped.get_or_insert(reason);
    quit.store(true, Ordering::SeqCst);
    std::mem::take(&mut state.pending)
}

/// Close admission under the same lock used by producers. The caller must
/// release native ownership and drop the returned captures outside this lock.
pub(crate) fn close_and_take_pending(queue: &Queue, quit: &AtomicBool) -> VecDeque<Closure> {
    let mut state = lock_queue(queue, quit);
    close_locked(&mut state, quit, StopReason::Requested)
}

pub(crate) fn queue_is_empty(queue: &Queue, quit: &AtomicBool) -> bool {
    lock_queue(queue, quit).pending.is_empty()
}

pub(crate) fn take_pending(queue: &Queue, quit: &AtomicBool) -> VecDeque<Closure> {
    std::mem::take(&mut lock_queue(queue, quit).pending)
}

pub(crate) fn failure(queue: &Queue, quit: &AtomicBool) -> Option<String> {
    match lock_queue(queue, quit).stopped {
        Some(StopReason::CapacityExceeded) => Some(format!(
            "Kindle event loop closure queue exceeded its {MAX_QUEUED_CLOSURES}-event capacity"
        )),
        Some(StopReason::Poisoned) => Some("Kindle event loop closure queue was poisoned".into()),
        Some(StopReason::Requested) | None => None,
    }
}

pub(crate) struct Wakeup {
    /// the event loop waits on this.
    pub(crate) read: Arc<OwnedFd>,
    /// proxies write a byte here to wake the loop.
    pub(crate) write: Arc<OwnedFd>,
}

pub(crate) struct KindleEventLoopProxy {
    pub(crate) queue: Queue,
    /// Keep a reader alive while an outstanding proxy can still poke the pipe.
    pub(crate) _read_fd: Arc<OwnedFd>,
    pub(crate) write_fd: Arc<OwnedFd>,
    pub(crate) quit_flag: Arc<AtomicBool>,
}

impl EventLoopProxy for KindleEventLoopProxy {
    fn quit_event_loop(&self) -> Result<(), EventLoopError> {
        let pending = close_and_take_pending(&self.queue, &self.quit_flag);
        poke(&self.write_fd);
        drop(pending);
        Ok(())
    }

    fn invoke_from_event_loop(&self, event: Closure) -> Result<(), EventLoopError> {
        let mut event = Some(event);
        let mut cancelled = VecDeque::new();
        let (accepted, wake) = {
            let mut state = lock_queue(&self.queue, &self.quit_flag);
            // Admission and quit share this lock. A producer which waited for
            // it must observe shutdown before it can retain another closure.
            if self.quit_flag.load(Ordering::SeqCst) || state.stopped.is_some() {
                let wake = matches!(state.stopped, Some(StopReason::Poisoned));
                cancelled = close_locked(&mut state, &self.quit_flag, StopReason::Requested);
                (false, wake)
            } else if state.pending.len() >= MAX_QUEUED_CLOSURES {
                // Opaque control closures cannot be safely evicted or merged.
                // Slint 1.17 has no queue-full error, so close the loop and let
                // it return the specific capacity failure through its cleanup.
                cancelled = close_locked(&mut state, &self.quit_flag, StopReason::CapacityExceeded);
                (false, true)
            } else {
                state.pending.push_back(event.take().expect("closure is present"));
                (true, true)
            }
        };
        if wake {
            poke(&self.write_fd);
        }
        // Captures can reenter this proxy or release a waiting channel. Never
        // run those destructors while holding the admission mutex.
        drop(cancelled);
        drop(event);
        if accepted { Ok(()) } else { Err(EventLoopError::EventLoopTerminated) }
    }
}

pub(crate) fn make_wakeup() -> std::io::Result<Wakeup> {
    let mut fds: [libc::c_int; 2] = [0; 2];
    // SAFETY: pipe() fills two fds or returns -1; we check for failure.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: pipe() gave us two new fds; wrap them now so they get closed
    // automatically if anything below fails.
    let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };

    set_nonblock_cloexec(&read)?;
    set_nonblock_cloexec(&write)?;

    Ok(Wakeup {
        read: Arc::new(read),
        write: Arc::new(write),
    })
}

fn set_nonblock_cloexec(file_descriptor: &OwnedFd) -> std::io::Result<()> {
    let raw = file_descriptor.as_raw_fd();
    // SAFETY: these fcntl variants take an int arg or none, and fd is valid for the borrow.
    unsafe {
        let flags = libc::fcntl(raw, libc::F_GETFL);
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let flags = libc::fcntl(raw, libc::F_GETFD);
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::fcntl(raw, libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Empty the pipe. Safe to call when there's nothing to read
pub(crate) fn drain(fd: &OwnedFd) {
    let mut buf = [0u8; 64];
    loop {
        // SAFETY: buf is valid for `buf.len()` bytes; fd is valid for the borrow.
        let n = unsafe {
            libc::read(
                fd.as_raw_fd(),
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if n < buf.len() as isize {
            break;
        }
    }
}

pub(crate) fn poke(fd: &OwnedFd) {
    let byte: u8 = 1;
    loop {
        // SAFETY: writing one byte to a pipe is always valid, and fd is valid for the borrow.
        let written = unsafe {
            libc::write(fd.as_raw_fd(), &byte as *const u8 as *const libc::c_void, 1)
        };
        if written == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        // EAGAIN means the pipe is already readable. Other failures cannot be
        // repaired by spinning here; the event loop owns shutdown detection.
        break;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Barrier, mpsc};
    use std::time::Duration;

    fn proxy() -> Arc<KindleEventLoopProxy> {
        let wakeup = make_wakeup().unwrap();
        Arc::new(KindleEventLoopProxy {
            queue: new_queue(),
            _read_fd: wakeup.read,
            write_fd: wakeup.write,
            quit_flag: Arc::new(AtomicBool::new(false)),
        })
    }

    struct Capture {
        queue: Queue,
        dropped: Arc<AtomicUsize>,
        verify_lock: bool,
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            if self.verify_lock {
                assert!(self.queue.try_lock().is_ok(), "capture was dropped under the queue lock");
            }
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn captured(proxy: &KindleEventLoopProxy, dropped: &Arc<AtomicUsize>) -> Closure {
        let capture = Capture { queue: proxy.queue.clone(), dropped: dropped.clone(), verify_lock: true };
        Box::new(move || drop(capture))
    }

    #[test]
    fn fifo_batch_frees_capacity_for_later_closures() {
        let proxy = proxy();
        let observed = Arc::new(Mutex::new(Vec::new()));
        for value in 0..MAX_QUEUED_CLOSURES {
            let observed = observed.clone();
            proxy.invoke_from_event_loop(Box::new(move || observed.lock().unwrap().push(value))).unwrap();
        }
        let batch = take_pending(&proxy.queue, &proxy.quit_flag);
        assert_eq!(batch.len(), MAX_QUEUED_CLOSURES);
        for event in batch { event(); }
        assert_eq!(*observed.lock().unwrap(), (0..MAX_QUEUED_CLOSURES).collect::<Vec<_>>());
        proxy.invoke_from_event_loop(Box::new(|| {})).unwrap();
        assert!(failure(&proxy.queue, &proxy.quit_flag).is_none());
        proxy.quit_event_loop().unwrap();
    }

    #[test]
    fn overflow_explicitly_closes_and_cancels_without_evicting_an_opaque_event() {
        let proxy = proxy();
        let dropped = Arc::new(AtomicUsize::new(0));
        for _ in 0..MAX_QUEUED_CLOSURES {
            proxy.invoke_from_event_loop(captured(&proxy, &dropped)).unwrap();
        }
        assert_eq!(
            proxy.invoke_from_event_loop(captured(&proxy, &dropped)),
            Err(EventLoopError::EventLoopTerminated)
        );
        assert!(proxy.quit_flag.load(Ordering::SeqCst));
        assert!(queue_is_empty(&proxy.queue, &proxy.quit_flag));
        assert!(failure(&proxy.queue, &proxy.quit_flag).unwrap().contains("capacity"));
        assert_eq!(dropped.load(Ordering::SeqCst), MAX_QUEUED_CLOSURES + 1);
    }

    #[test]
    fn producer_waiting_on_the_admission_mutex_observes_shutdown() {
        let proxy = proxy();
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut state = proxy.queue.lock().unwrap();
        let (started, ready) = mpsc::channel();
        let producer = proxy.clone();
        let event = captured(&proxy, &dropped);
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            producer.invoke_from_event_loop(event)
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let pending = close_locked(&mut state, &proxy.quit_flag, StopReason::Requested);
        drop(state);
        drop(pending);
        assert_eq!(worker.join().unwrap(), Err(EventLoopError::EventLoopTerminated));
        assert!(queue_is_empty(&proxy.queue, &proxy.quit_flag));
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn concurrent_quit_releases_all_captures_after_producer_races() {
        const PRODUCERS: usize = 32;
        let proxy = proxy();
        let dropped = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(Barrier::new(PRODUCERS + 2));
        let mut workers = Vec::new();
        for _ in 0..PRODUCERS {
            let producer = proxy.clone();
            let start = start.clone();
            // Another producer may legitimately own the mutex when this
            // capture drops. Lock-free destruction is checked separately in
            // single-producer tests; this race checks eventual cancellation.
            let capture = Capture { queue: proxy.queue.clone(), dropped: dropped.clone(), verify_lock: false };
            let event: Closure = Box::new(move || drop(capture));
            workers.push(std::thread::spawn(move || {
                start.wait();
                producer.invoke_from_event_loop(event)
            }));
        }
        let quitter = proxy.clone();
        let quit_start = start.clone();
        let quit = std::thread::spawn(move || {
            quit_start.wait();
            quitter.quit_event_loop().unwrap();
        });
        start.wait();
        for worker in workers {
            match worker.join().unwrap() {
                Ok(()) | Err(EventLoopError::EventLoopTerminated) => {}
                other => panic!("unexpected queue result: {other:?}"),
            }
        }
        quit.join().unwrap();
        assert!(queue_is_empty(&proxy.queue, &proxy.quit_flag));
        assert_eq!(dropped.load(Ordering::SeqCst), PRODUCERS);
        assert_eq!(proxy.invoke_from_event_loop(Box::new(|| {})), Err(EventLoopError::EventLoopTerminated));
    }

    #[test]
    fn quit_disconnects_a_channel_captured_by_unexecuted_work() {
        let proxy = proxy();
        let (sender, receiver) = mpsc::channel::<()>();
        proxy.invoke_from_event_loop(Box::new(move || drop(sender))).unwrap();
        proxy.quit_event_loop().unwrap();
        assert_eq!(receiver.recv_timeout(Duration::from_secs(2)), Err(mpsc::RecvTimeoutError::Disconnected));
    }

    #[test]
    fn a_poisoned_queue_fails_closed_and_can_still_release_captures() {
        let proxy = proxy();
        let (sender, receiver) = mpsc::channel::<()>();
        proxy.invoke_from_event_loop(Box::new(move || drop(sender))).unwrap();
        let queue = proxy.queue.clone();
        assert!(std::thread::spawn(move || {
            let _guard = queue.lock().unwrap();
            panic!("controlled admission failure");
        }).join().is_err());
        assert_eq!(proxy.invoke_from_event_loop(Box::new(|| {})), Err(EventLoopError::EventLoopTerminated));
        assert_eq!(receiver.recv_timeout(Duration::from_secs(2)), Err(mpsc::RecvTimeoutError::Disconnected));
        assert!(failure(&proxy.queue, &proxy.quit_flag).unwrap().contains("poisoned"));
    }
}
