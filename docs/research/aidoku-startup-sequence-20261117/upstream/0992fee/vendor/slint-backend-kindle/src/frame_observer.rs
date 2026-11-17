//! Opt-in, one-shot observations of accepted Reader framebuffer output.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use crate::PhysicalRect;
use crate::framebuffer::FrameDitheringStatus;

/// A fixed outcome code; neither device errors nor application content is exposed.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum NativeFrameUnavailable {
    NoWrite,
    SendRejected,
    Superseded,
    Cancelled,
}

impl NativeFrameUnavailable {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoWrite => "no_write",
            Self::SendRejected => "send_rejected",
            Self::Superseded => "superseded",
            Self::Cancelled => "cancelled",
        }
    }
}

/// A successful SEND is driver acceptance, not independent panel completion.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum NativeFramePhase {
    FramebufferWritten,
    SendAccepted,
    Unavailable(NativeFrameUnavailable),
}

/// An observation contains an opaque application token and no document identity.
#[derive(Debug, Clone, Copy)]
pub struct NativeFrameObservation {
    pub token: u64,
    /// The actual framebuffer-write counter; zero means no write occurred.
    pub frame_sequence: u64,
    /// A fallback replacement links its accepted write to the first observed
    /// raw write without reusing that write's physical identity.
    pub replaces_frame_sequence: Option<u64>,
    pub phase: NativeFramePhase,
    /// Time spent in the observed write or submission call, excluding callbacks.
    pub elapsed: Duration,
    pub render_elapsed: Duration,
    pub region: Option<PhysicalRect>,
    pub full_frame: bool,
    pub marker: Option<u32>,
    pub hardware_y4_available: bool,
    pub hardware_y4_requested: bool,
    /// True only when this observation's accepted marker carries MTK Y4.
    pub hardware_y4_applied: bool,
    pub software_dither_fallback: bool,
    pub dithering_delivery_uncertain: bool,
}

type Observer = Rc<dyn Fn(NativeFrameObservation)>;

/// Startup checkpoints are independent of the one-shot Reader token protocol.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum NativeCheckpointPhase {
    Submitted,
    InputReady,
}

#[derive(Debug, Clone, Copy)]
pub struct NativeFrameCheckpoint {
    pub phase: NativeCheckpointPhase,
    pub frame_sequence: u64,
    pub marker: Option<u32>,
    pub initial: bool,
    pub full: bool,
    pub active: bool,
    pub input_blocked: bool,
    pub render_elapsed: Duration,
    pub settle_elapsed: Duration,
    pub write_elapsed: Duration,
    pub submit_elapsed: Duration,
}

type CheckpointObserver = Rc<dyn Fn(NativeFrameCheckpoint)>;

/// Initial scene allocation may require a full write of the first accepted
/// Reader image. Later ownership, rotation or output-mode rewrites cannot be
/// reused as page-turn completions; a resume is never an initial-start sample.
/// `presentation_rewrite` excludes the dithering policy of the armed image:
/// processing that same image may legitimately require its first full write.
pub(crate) fn reader_output_eligible(
    frame_sequence: u64,
    resume_pending: bool,
    presentation_rewrite: bool,
) -> bool {
    !resume_pending && (!presentation_rewrite || frame_sequence == 1)
}

#[derive(Default)]
pub(crate) struct FrameObserverState {
    observer: RefCell<Option<Observer>>,
    checkpoint_observer: RefCell<Option<CheckpointObserver>>,
    checkpoint_timing: Cell<bool>,
    pending: Cell<Option<u64>>,
    revision: Cell<u64>,
    invalidated_by: Cell<Option<NativeFrameUnavailable>>,
}

impl FrameObserverState {
    pub(crate) fn set_checkpoint_observer(
        &self,
        observer: impl Fn(NativeFrameCheckpoint) + 'static,
        timing: bool,
    ) {
        self.checkpoint_observer.replace(Some(Rc::new(observer)));
        self.checkpoint_timing.set(timing);
    }

    pub(crate) fn checkpoint_timing(&self) -> bool {
        self.checkpoint_timing.get()
    }

    pub(crate) fn checkpoint(&self, checkpoint: NativeFrameCheckpoint) {
        let observer = self.checkpoint_observer.borrow().clone();
        if let Some(observer) = observer {
            observer(checkpoint);
        }
    }

    pub(crate) fn set_observer(&self, observer: impl Fn(NativeFrameObservation) + 'static) {
        self.cancel();
        self.observer.replace(Some(Rc::new(observer)));
    }

    /// Replace an unconsumed token. An active attempt also becomes stale.
    pub(crate) fn arm(&self, token: u64) -> Option<u64> {
        if self.observer.borrow().is_none() {
            return None;
        }
        self.invalidate(NativeFrameUnavailable::Superseded);
        self.pending.replace(Some(token))
    }

    pub(crate) fn cancel(&self) -> Option<u64> {
        self.invalidate(NativeFrameUnavailable::Cancelled);
        self.pending.take()
    }

    fn invalidate(&self, reason: NativeFrameUnavailable) {
        self.revision.set(self.revision.get().wrapping_add(1));
        self.invalidated_by.set(Some(reason));
    }

    /// Consume the token before rendering so a later unrelated draw cannot reuse it.
    pub(crate) fn begin(self: &Rc<Self>) -> Option<FrameAttempt> {
        let token = self.pending.take()?;
        let observer = self.observer.borrow().clone()?;
        Some(FrameAttempt {
            state: self.clone(),
            observer,
            revision: self.revision.get(),
            observation: NativeFrameObservation {
                token,
                frame_sequence: 0,
                replaces_frame_sequence: None,
                phase: NativeFramePhase::Unavailable(NativeFrameUnavailable::NoWrite),
                elapsed: Duration::ZERO,
                render_elapsed: Duration::ZERO,
                region: None,
                full_frame: false,
                marker: None,
                hardware_y4_available: false,
                hardware_y4_requested: false,
                hardware_y4_applied: false,
                software_dither_fallback: false,
                dithering_delivery_uncertain: false,
            },
            wrote: false,
            finished: false,
        })
    }
}

pub(crate) struct FrameAttempt {
    state: Rc<FrameObserverState>,
    observer: Observer,
    revision: u64,
    observation: NativeFrameObservation,
    wrote: bool,
    finished: bool,
}

impl FrameAttempt {
    pub(crate) fn set_dithering_status(&mut self, status: FrameDitheringStatus) {
        self.observation.hardware_y4_available = status.hardware_y4_available;
        self.observation.hardware_y4_requested = status.hardware_y4_requested;
        self.observation.hardware_y4_applied = status.hardware_y4_applied;
        self.observation.software_dither_fallback = status.software_dither_fallback;
        self.observation.dithering_delivery_uncertain = status.dithering_delivery_uncertain;
    }

    /// A definite hardware rejection may require one additional full write.
    /// Preserve the first write event while binding acceptance to its replacement.
    pub(crate) fn rewritten(&mut self, frame_sequence: u64, region: PhysicalRect) {
        if self.current() && self.wrote {
            self.observation.replaces_frame_sequence = Some(self.observation.frame_sequence);
            self.observation.frame_sequence = frame_sequence;
            self.observation.region = Some(region);
            self.observation.full_frame = true;
        }
    }

    fn current(&mut self) -> bool {
        if self.finished {
            return false;
        }
        if self.state.revision.get() != self.revision {
            self.unavailable(
                self.state
                    .invalidated_by
                    .get()
                    .unwrap_or(NativeFrameUnavailable::Cancelled),
            );
            return false;
        }
        true
    }

    pub(crate) fn written(
        &mut self,
        frame_sequence: u64,
        region: PhysicalRect,
        full_frame: bool,
        elapsed: Duration,
        render_elapsed: Duration,
    ) {
        if !self.current() || self.wrote {
            return;
        }
        self.wrote = true;
        self.observation.frame_sequence = frame_sequence;
        self.observation.phase = NativeFramePhase::FramebufferWritten;
        self.observation.hardware_y4_applied = false;
        self.observation.elapsed = elapsed;
        self.observation.render_elapsed = render_elapsed;
        self.observation.region = Some(region);
        self.observation.full_frame = full_frame;
        // The callback is cloned before invocation; no RefCell borrow or mutex
        // remains held, including when a callback invalidates this attempt.
        (self.observer)(self.observation);
    }

    pub(crate) fn accepted(&mut self, marker: Option<u32>, elapsed: Duration) {
        if !self.current() {
            return;
        }
        if !self.wrote {
            self.unavailable(NativeFrameUnavailable::NoWrite);
            return;
        }
        if self.observation.dithering_delivery_uncertain {
            self.unavailable(NativeFrameUnavailable::SendRejected);
            return;
        }
        let Some(marker) = marker.filter(|marker| *marker != 0) else {
            self.unavailable(NativeFrameUnavailable::SendRejected);
            return;
        };
        self.finished = true;
        self.observation.phase = NativeFramePhase::SendAccepted;
        self.observation.elapsed = elapsed;
        self.observation.marker = Some(marker);
        (self.observer)(self.observation);
    }

    pub(crate) fn unavailable(&mut self, reason: NativeFrameUnavailable) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.observation.phase = NativeFramePhase::Unavailable(reason);
        self.observation.elapsed = Duration::ZERO;
        self.observation.marker = None;
        self.observation.hardware_y4_applied = false;
        (self.observer)(self.observation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed() -> (
        Rc<FrameObserverState>,
        Rc<RefCell<Vec<NativeFrameObservation>>>,
    ) {
        let state = Rc::new(FrameObserverState::default());
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        state.set_observer(move |event| observed.borrow_mut().push(event));
        (state, events)
    }

    fn write(attempt: &mut FrameAttempt) {
        attempt.written(
            7,
            PhysicalRect {
                x: 2,
                y: 3,
                width: 10,
                height: 20,
            },
            false,
            Duration::from_micros(20),
            Duration::from_micros(30),
        );
    }

    #[test]
    fn an_unobserved_backend_does_not_arm_or_create_an_attempt() {
        let state = Rc::new(FrameObserverState::default());
        assert_eq!(state.arm(5), None);
        assert!(state.begin().is_none());
    }

    #[test]
    fn startup_observation_can_replace_itself_without_consuming_reader_identity() {
        let (state, events) = observed();
        state.arm(11);
        let weak = Rc::downgrade(&state);
        let calls = Rc::new(Cell::new(0));
        let observed_calls = calls.clone();
        state.set_checkpoint_observer(
            move |_| {
                observed_calls.set(observed_calls.get() + 1);
                weak.upgrade()
                    .unwrap()
                    .set_checkpoint_observer(|_| {}, false);
            },
            true,
        );
        assert!(state.checkpoint_timing());
        let checkpoint = NativeFrameCheckpoint {
            phase: NativeCheckpointPhase::Submitted,
            frame_sequence: 1,
            marker: Some(1),
            initial: true,
            full: true,
            active: true,
            input_blocked: true,
            render_elapsed: Duration::ZERO,
            settle_elapsed: Duration::ZERO,
            write_elapsed: Duration::ZERO,
            submit_elapsed: Duration::ZERO,
        };
        state.checkpoint(checkpoint);
        state.checkpoint(checkpoint);
        assert_eq!(calls.get(), 1);
        assert!(!state.checkpoint_timing());
        let mut attempt = state.begin().unwrap();
        write(&mut attempt);
        attempt.accepted(Some(2), Duration::ZERO);
        assert!(events.borrow().iter().all(|event| event.token == 11));
        assert_eq!(events.borrow().len(), 2);
    }

    #[test]
    fn one_shot_identity_survives_write_and_send_but_never_a_later_draw() {
        let (state, events) = observed();
        assert_eq!(state.arm(11), None);
        let mut attempt = state.begin().unwrap();
        assert!(state.begin().is_none());
        write(&mut attempt);
        attempt.accepted(Some(1), Duration::from_micros(40));
        attempt.accepted(Some(2), Duration::from_micros(50));
        assert!(state.begin().is_none());
        let events = events.borrow();
        assert_eq!(events.len(), 2);
        assert!(
            events
                .iter()
                .all(|event| event.token == 11 && event.frame_sequence == 7)
        );
        assert_eq!(events[0].phase, NativeFramePhase::FramebufferWritten);
        assert_eq!(events[0].elapsed, Duration::from_micros(20));
        assert_eq!(events[0].render_elapsed, Duration::from_micros(30));
        assert_eq!(events[1].phase, NativeFramePhase::SendAccepted);
        assert_eq!(events[1].marker, Some(1));
        assert_eq!(events[1].elapsed, Duration::from_micros(40));
    }

    #[test]
    fn replacing_or_cancelling_a_pending_token_returns_its_exact_identity() {
        let (state, events) = observed();
        assert_eq!(state.arm(11), None);
        assert_eq!(state.arm(12), Some(11));
        assert_eq!(state.cancel(), Some(12));
        assert_eq!(state.cancel(), None);
        assert!(state.begin().is_none());
        assert!(events.borrow().is_empty());
    }

    #[test]
    fn an_invalidation_after_begin_cannot_publish_the_old_frame_as_completed() {
        let (state, events) = observed();
        state.arm(11);
        let mut attempt = state.begin().unwrap();
        state.arm(12);
        write(&mut attempt);
        attempt.accepted(Some(1), Duration::ZERO);
        let events = events.borrow();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].token, 11);
        assert_eq!(
            events[0].phase,
            NativeFramePhase::Unavailable(NativeFrameUnavailable::Superseded)
        );
        assert_eq!(state.begin().unwrap().observation.token, 12);
    }

    #[test]
    fn a_reentrant_observer_can_cancel_without_borrowing_or_accepting_stale_output() {
        let state = Rc::new(FrameObserverState::default());
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        let weak = Rc::downgrade(&state);
        state.set_observer(move |event| {
            observed.borrow_mut().push(event);
            if event.phase == NativeFramePhase::FramebufferWritten {
                weak.upgrade().unwrap().cancel();
            }
        });
        state.arm(11);
        let mut attempt = state.begin().unwrap();
        write(&mut attempt);
        attempt.accepted(Some(1), Duration::ZERO);
        let events = events.borrow();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[1].phase,
            NativeFramePhase::Unavailable(NativeFrameUnavailable::Cancelled)
        );
    }

    #[test]
    fn no_write_failed_send_and_missing_marker_never_report_success() {
        for reason in [
            NativeFrameUnavailable::NoWrite,
            NativeFrameUnavailable::SendRejected,
            NativeFrameUnavailable::Cancelled,
        ] {
            let (state, events) = observed();
            state.arm(11);
            let mut attempt = state.begin().unwrap();
            if reason == NativeFrameUnavailable::SendRejected {
                write(&mut attempt);
            }
            attempt.unavailable(reason);
            attempt.accepted(Some(1), Duration::ZERO);
            assert_eq!(
                events.borrow().last().unwrap().phase,
                NativeFramePhase::Unavailable(reason)
            );
        }
        let (state, events) = observed();
        state.arm(11);
        let mut attempt = state.begin().unwrap();
        write(&mut attempt);
        attempt.accepted(Some(0), Duration::ZERO);
        assert_eq!(
            events.borrow().last().unwrap().phase,
            NativeFramePhase::Unavailable(NativeFrameUnavailable::SendRejected)
        );
    }

    #[test]
    fn dithering_is_bound_to_the_accepted_replacement_and_marker() {
        let (state, events) = observed();
        state.arm(11);
        let mut attempt = state.begin().unwrap();
        attempt.set_dithering_status(FrameDitheringStatus {
            hardware_y4_available: true,
            hardware_y4_requested: true,
            ..FrameDitheringStatus::default()
        });
        write(&mut attempt);
        attempt.rewritten(
            8,
            PhysicalRect {
                x: 0,
                y: 0,
                width: 100,
                height: 200,
            },
        );
        attempt.set_dithering_status(FrameDitheringStatus {
            hardware_y4_requested: true,
            software_dither_fallback: true,
            ..FrameDitheringStatus::default()
        });
        attempt.accepted(Some(3), Duration::from_micros(40));
        let events = events.borrow();
        assert_eq!(events.len(), 2);
        assert_eq!((events[0].frame_sequence, events[0].marker), (7, None));
        assert_eq!(events[0].replaces_frame_sequence, None);
        assert!(events[0].hardware_y4_available && events[0].hardware_y4_requested);
        assert!(!events[0].hardware_y4_applied);
        assert_eq!((events[1].frame_sequence, events[1].marker), (8, Some(3)));
        assert_eq!(events[1].replaces_frame_sequence, Some(7));
        assert!(events[1].full_frame && events[1].software_dither_fallback);
        assert!(!events[1].hardware_y4_available && !events[1].hardware_y4_applied);
    }

    #[test]
    fn uncertain_or_missing_submissions_cannot_report_applied_hardware() {
        for marker in [None, Some(0), Some(1)] {
            let (state, events) = observed();
            state.arm(11);
            let mut attempt = state.begin().unwrap();
            write(&mut attempt);
            attempt.set_dithering_status(FrameDitheringStatus {
                hardware_y4_available: true,
                hardware_y4_requested: true,
                hardware_y4_applied: true,
                dithering_delivery_uncertain: true,
                ..FrameDitheringStatus::default()
            });
            attempt.accepted(marker, Duration::ZERO);
            let event = *events.borrow().last().unwrap();
            assert_eq!(
                event.phase,
                NativeFramePhase::Unavailable(NativeFrameUnavailable::SendRejected)
            );
            assert!(!event.hardware_y4_applied && event.dithering_delivery_uncertain);
            assert_eq!(event.marker, None);
        }
    }

    #[test]
    fn first_reader_allocation_is_measurable_but_later_rewrites_and_resume_are_not() {
        for (sequence, resume, rewrite, accepted) in [
            (1, false, true, true),
            (1, false, false, true),
            (2, false, false, true),
            (2, false, true, false),
            (1, true, true, false),
            (2, true, true, false),
            (2, true, false, false),
        ] {
            let (state, events) = observed();
            state.arm(11);
            let mut attempt = state.begin().unwrap();
            if reader_output_eligible(sequence, resume, rewrite) {
                attempt.written(
                    sequence,
                    PhysicalRect {
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 200,
                    },
                    true,
                    Duration::ZERO,
                    Duration::ZERO,
                );
                attempt.accepted(Some(1), Duration::ZERO);
            } else {
                attempt.unavailable(NativeFrameUnavailable::Cancelled);
            }
            let completed = events
                .borrow()
                .iter()
                .any(|event| event.phase == NativeFramePhase::SendAccepted);
            assert_eq!(
                completed, accepted,
                "sequence={sequence}, resume={resume}, rewrite={rewrite}"
            );
        }
    }
}
