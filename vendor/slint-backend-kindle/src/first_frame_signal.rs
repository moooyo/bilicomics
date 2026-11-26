//! One-shot admission for background preparation after the initial full SEND.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::Rc;
use std::sync::mpsc::Sender;

/// A first-frame signal can be armed once, before its submission or retirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FirstFrameSignalError {
    AlreadyArmed,
    Retired,
}

impl fmt::Display for FirstFrameSignalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AlreadyArmed => "the first-frame signal is already armed",
            Self::Retired => "the first-frame signal has been retired",
        })
    }
}

impl std::error::Error for FirstFrameSignalError {}

/// A validated initial full SEND, not a completion or input-readiness signal.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FirstFrameNotice {
    /// Existing submit clock when observation is enabled; never read for the notice alone.
    pub accepted_monotonic_ns: Option<u64>,
}

#[derive(Default)]
struct Signal {
    sender: RefCell<Option<Sender<FirstFrameNotice>>>,
    retired: Cell<bool>,
}

/// A UI-thread-only, one-shot background notification with no application callback.
///
/// The signal is disabled until armed. Its receiver may start background byte or
/// metadata preparation; UI attachment must keep its normal event-loop boundary.
/// Submission is not driver completion, input readiness, or optical panel evidence.
#[derive(Clone, Default)]
pub struct FirstFrameSignalHandle {
    inner: Rc<Signal>,
}

impl FirstFrameSignalHandle {
    /// Arm before showing the window or entering the event loop.
    ///
    /// Only the first accepted initial full-frame submission can consume this
    /// sender. An already observed submission or explicit retirement is final.
    pub fn arm(&self, sender: Sender<FirstFrameNotice>) -> Result<(), FirstFrameSignalError> {
        if self.inner.retired.get() {
            return Err(FirstFrameSignalError::Retired);
        }
        let mut slot = self.inner.sender.borrow_mut();
        if slot.is_some() {
            return Err(FirstFrameSignalError::AlreadyArmed);
        }
        *slot = Some(sender);
        Ok(())
    }

    /// Permanently close admission and disconnect any waiting receiver.
    ///
    /// This is idempotent. Owners must also retire on errors before event-loop
    /// entry, so background waiters cannot retain runtime shutdown indefinitely.
    pub fn retire(&self) {
        self.inner.retired.set(true);
        let sender = self.inner.sender.borrow_mut().take();
        drop(sender);
    }

    /// The caller has proved a real, accepted initial full-frame submission.
    /// The renderer borrow has ended; this never invokes application code.
    pub(crate) fn submitted(&self, notice: FirstFrameNotice) {
        if self.inner.retired.replace(true) {
            return;
        }
        let sender = self.inner.sender.borrow_mut().take();
        if let Some(sender) = sender {
            // An unbounded Sender never waits for the receiver to consume it.
            let _ = sender.send(notice);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, TryRecvError};

    #[test]
    fn one_submission_consumes_all_clones_and_cannot_be_rearmed() {
        let signal = FirstFrameSignalHandle::default();
        let clone = signal.clone();
        let (sender, receiver) = mpsc::channel();
        signal.arm(sender).unwrap();
        let (duplicate, duplicate_receiver) = mpsc::channel();
        assert_eq!(clone.arm(duplicate), Err(FirstFrameSignalError::AlreadyArmed));
        assert_eq!(duplicate_receiver.try_recv(), Err(TryRecvError::Disconnected));

        let notice = FirstFrameNotice { accepted_monotonic_ns: Some(123) };
        clone.submitted(notice);
        assert_eq!(receiver.try_recv(), Ok(notice));
        signal.submitted(FirstFrameNotice::default());
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Disconnected));
        let (late, _) = mpsc::channel();
        assert_eq!(signal.arm(late), Err(FirstFrameSignalError::Retired));
    }

    #[test]
    fn an_unarmed_submission_prevents_late_admission() {
        let signal = FirstFrameSignalHandle::default();
        signal.submitted(FirstFrameNotice::default());
        let (sender, receiver) = mpsc::channel();
        assert_eq!(signal.arm(sender), Err(FirstFrameSignalError::Retired));
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Disconnected));
    }

    #[test]
    fn retirement_disconnects_waiters_without_publishing_a_submission() {
        let signal = FirstFrameSignalHandle::default();
        let (sender, receiver) = mpsc::channel();
        signal.arm(sender).unwrap();
        signal.retire();
        signal.retire();
        signal.submitted(FirstFrameNotice::default());
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Disconnected));
        let (late, _) = mpsc::channel();
        assert_eq!(signal.arm(late), Err(FirstFrameSignalError::Retired));
    }

    #[test]
    fn a_closed_receiver_does_not_retry_the_submission() {
        let signal = FirstFrameSignalHandle::default();
        let (sender, receiver) = mpsc::channel();
        signal.arm(sender).unwrap();
        drop(receiver);
        signal.submitted(FirstFrameNotice::default());
        let (late, _) = mpsc::channel();
        assert_eq!(signal.arm(late), Err(FirstFrameSignalError::Retired));
    }
}
