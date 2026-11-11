//! Completion tokens for application-owned sleep frames on the UI thread.

use std::{cell::Cell, io};

#[derive(Clone, Copy, Default)]
struct State {
    requested: u64,
    completed: u64,
    cancelled_through: u64,
    terminated: bool,
}

#[derive(Default)]
pub(crate) struct SleepPresentationControl(Cell<State>);

fn unavailable(message: &'static str) -> io::Error {
    io::Error::other(message)
}

impl SleepPresentationControl {
    pub(crate) fn request(&self) -> io::Result<u64> {
        let mut state = self.0.get();
        if state.terminated {
            return Err(unavailable("the Kindle event loop has terminated"));
        }
        state.requested = state.requested.checked_add(1)
            .ok_or_else(|| unavailable("the sleep presentation sequence is exhausted"))?;
        self.0.set(state);
        Ok(state.requested)
    }

    pub(crate) fn pending(&self) -> io::Result<Option<u64>> {
        let state = self.0.get();
        if state.terminated {
            return Err(unavailable("the Kindle event loop has terminated"));
        }
        Ok((state.requested > state.completed && state.requested > state.cancelled_through)
            .then_some(state.requested))
    }

    pub(crate) fn completed(&self, token: u64) -> io::Result<bool> {
        let state = self.checked(token)?;
        Ok(token <= state.completed)
    }

    fn checked(&self, token: u64) -> io::Result<State> {
        let state = self.0.get();
        if state.terminated {
            return Err(unavailable("the Kindle event loop has terminated"));
        }
        if token == 0 || token > state.requested {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid sleep presentation token"));
        }
        if token <= state.cancelled_through {
            return Err(unavailable("the sleep presentation was cancelled"));
        }
        Ok(state)
    }

    /// Only acknowledge the token captured before drawing, after its checked
    /// full update has completed. A request arriving later needs another frame.
    pub(crate) fn acknowledge(&self, token: u64) -> io::Result<()> {
        let mut state = self.checked(token)?;
        state.completed = state.completed.max(token);
        self.0.set(state);
        Ok(())
    }

    pub(crate) fn cancel_pending(&self) {
        let mut state = self.0.get();
        if state.requested > state.completed {
            state.cancelled_through = state.requested;
        }
        self.0.set(state);
    }

    pub(crate) fn terminate(&self) {
        let mut state = self.0.get();
        state.terminated = true;
        self.0.set(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_only_completes_tokens_requested_before_it_was_captured() {
        let control = SleepPresentationControl::default();
        assert_eq!(control.pending().unwrap(), None);
        let first = control.request().unwrap();
        let captured = control.request().unwrap();
        assert!(!control.completed(first).unwrap());
        let later = control.request().unwrap();
        control.acknowledge(captured).unwrap();
        assert!(control.completed(first).unwrap());
        assert!(control.completed(captured).unwrap());
        assert!(!control.completed(later).unwrap());
        assert_eq!(control.pending().unwrap(), Some(later));
    }

    #[test]
    fn an_interrupted_request_cannot_be_completed_by_a_wake_frame() {
        let control = SleepPresentationControl::default();
        let cancelled = control.request().unwrap();
        control.cancel_pending();
        assert!(control.completed(cancelled).is_err());
        assert!(control.acknowledge(cancelled).is_err());
        assert_eq!(control.pending().unwrap(), None);
        let fresh = control.request().unwrap();
        control.acknowledge(fresh).unwrap();
        assert!(control.completed(fresh).unwrap());
    }

    #[test]
    fn invalid_tokens_overflow_and_termination_never_report_success() {
        let control = SleepPresentationControl::default();
        assert!(control.completed(0).is_err());
        assert!(control.completed(1).is_err());
        control.0.set(State { requested: u64::MAX, ..State::default() });
        assert!(control.request().is_err());
        control.acknowledge(u64::MAX).unwrap();
        assert!(control.completed(u64::MAX).unwrap());
        control.terminate();
        assert!(control.pending().is_err());
        assert!(control.completed(u64::MAX).is_err());
        assert!(control.request().is_err());
        assert!(control.acknowledge(u64::MAX).is_err());
    }
}
