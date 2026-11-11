//! Yield to the renderer before any synchronous sleep checkpoint work.

use std::time::{Duration, Instant};

use anyhow::Result;

const PRESENTATION_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) enum CoverProgress {
    Stock,
    Pending,
    Presented(Duration),
}

struct Pending<Request> {
    request: Request,
    token: u64,
    started: Instant,
}

pub(crate) struct SleepCoverRuntime<Request> {
    pending: Option<Pending<Request>>,
}

impl<Request> Default for SleepCoverRuntime<Request> {
    fn default() -> Self { Self { pending: None } }
}

impl<Request: Eq> SleepCoverRuntime<Request> {
    pub(crate) fn cancel(&mut self) { self.pending = None; }

    /// Only a completion for the same current request authorizes checkpoint work.
    /// The caller hides its temporary overlay on Stock and on every error.
    pub(crate) fn poll(
        &mut self,
        request: Option<Request>,
        now: Instant,
        present: impl FnOnce() -> Result<u64>,
        completed: impl FnOnce(u64) -> Result<bool>,
    ) -> Result<CoverProgress> {
        let Some(request) = request else {
            self.cancel();
            return Ok(CoverProgress::Stock);
        };
        if self.pending.as_ref().is_some_and(|pending| pending.request == request) {
            let pending = self.pending.as_ref().expect("The matching request exists");
            let elapsed = now.saturating_duration_since(pending.started);
            if elapsed >= PRESENTATION_TIMEOUT {
                self.cancel();
                return Ok(CoverProgress::Stock);
            }
            let completion = completed(pending.token);
            match completion {
                Ok(false) => Ok(CoverProgress::Pending),
                Ok(true) => {
                    self.cancel();
                    Ok(CoverProgress::Presented(elapsed))
                }
                Err(error) => {
                    self.cancel();
                    Err(error)
                }
            }
        } else {
            self.cancel();
            let token = present()?;
            self.pending = Some(Pending { request, token, started: now });
            Ok(CoverProgress::Pending)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use anyhow::bail;

    use super::*;

    #[test]
    fn a_cover_must_yield_then_observe_its_own_completion_before_checkpoint_work() {
        let mut state = SleepCoverRuntime::default();
        let started = Instant::now();
        let completion_checks = Cell::new(0);
        let first = state.poll(Some(4), started, || Ok(37), |_| {
            completion_checks.set(completion_checks.get() + 1);
            Ok(true)
        }).unwrap();
        assert!(matches!(first, CoverProgress::Pending));
        assert_eq!(completion_checks.get(), 0);
        let waiting = state.poll(Some(4), started + Duration::from_millis(80),
            || panic!("The same cover must not be presented twice"), |token| {
                assert_eq!(token, 37);
                Ok(false)
            }).unwrap();
        assert!(matches!(waiting, CoverProgress::Pending));
        let ready = state.poll(Some(4), started + Duration::from_millis(160),
            || panic!("A completed cover must not be presented twice"), |token| {
                assert_eq!(token, 37);
                Ok(true)
            }).unwrap();
        assert!(matches!(ready, CoverProgress::Presented(elapsed) if elapsed == Duration::from_millis(160)));
    }

    #[test]
    fn a_new_request_never_accepts_the_old_token() {
        let mut state = SleepCoverRuntime::default();
        let now = Instant::now();
        state.poll(Some(1), now, || Ok(11), |_| unreachable!()).unwrap();
        let next = state.poll(Some(2), now, || Ok(22), |_| panic!("The old request cannot be acknowledged")).unwrap();
        assert!(matches!(next, CoverProgress::Pending));
        state.poll(Some(2), now, || unreachable!(), |token| { assert_eq!(token, 22); Ok(true) }).unwrap();
    }

    #[test]
    fn timeout_missing_intent_and_renderer_errors_return_without_cover_success() {
        let now = Instant::now();
        let mut state = SleepCoverRuntime::default();
        state.poll(Some(1), now, || Ok(1), |_| unreachable!()).unwrap();
        assert!(matches!(state.poll(Some(1), now + PRESENTATION_TIMEOUT,
            || unreachable!(), |_| panic!("A timed-out cover cannot acknowledge late completion")).unwrap(), CoverProgress::Stock));
        assert!(state.pending.is_none());
        state.poll(Some(2), now, || Ok(2), |_| unreachable!()).unwrap();
        assert!(matches!(state.poll(None, now, || unreachable!(), |_| unreachable!()).unwrap(), CoverProgress::Stock));
        state.poll(Some(3), now, || Ok(3), |_| unreachable!()).unwrap();
        assert!(state.poll(Some(3), now, || unreachable!(), |_| bail!("Injected update wait failure")).is_err());
        assert!(state.pending.is_none());
        assert!(state.poll(Some(4), now, || bail!("Injected update request failure"), |_| unreachable!()).is_err());
        assert!(state.pending.is_none());
    }
}
