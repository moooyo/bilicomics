//! Opt-in observation of the first qualifying startup Shelf frame.

use std::cell::Cell;

use crate::{PhysicalRect, framebuffer::{Framebuffer, RefreshSubmission}, performance::monotonic_ns};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShelfState {
    Ready,
    Empty,
    SignedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShelfRenderScene {
    Booting,
    Blocked,
    Other,
    Shelf(ShelfState),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShelfRenderOrder {
    InitialFill,
    WaitFirst,
    PrivateRenderFirst,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShelfRenderSample {
    pub shelf_state: ShelfState,
    pub render_sequence: u64,
    pub render_order: ShelfRenderOrder,
    pub rendered_monotonic_ns: Option<u64>,
    pub submission: Option<ShelfRenderSubmission>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShelfRenderSubmission {
    pub update_marker: u32,
    pub submitted_monotonic_ns: u64,
    pub completed_monotonic_ns: Option<u64>,
}

pub(crate) struct ShelfRenderTrace {
    scene: Box<dyn Fn() -> ShelfRenderScene>,
    emit: Box<dyn Fn(ShelfRenderSample)>,
    sequence: Cell<Option<u64>>,
}

impl ShelfRenderTrace {
    pub(crate) fn new(scene: impl Fn() -> ShelfRenderScene + 'static, emit: impl Fn(ShelfRenderSample) + 'static) -> Self {
        Self { scene: Box::new(scene), emit: Box::new(emit), sequence: Cell::new(Some(0)) }
    }

    pub(crate) fn retire(&self) {
        self.sequence.set(None);
    }

    pub(crate) fn candidate(&self) -> Option<ShelfRenderCandidate> {
        let render_sequence = self.sequence.get()?.checked_add(1)?;
        match (self.scene)() {
            ShelfRenderScene::Shelf(shelf_state) => {
                self.sequence.set(Some(render_sequence));
                Some(ShelfRenderCandidate { shelf_state, render_sequence })
            }
            ShelfRenderScene::Other => {
                self.retire();
                None
            }
            _ => None,
        }
    }

    pub(crate) fn emit(&self, sample: ShelfRenderSample) {
        if self.sequence.take() == Some(sample.render_sequence)
            && (self.scene)() == ShelfRenderScene::Shelf(sample.shelf_state)
        {
            (self.emit)(sample);
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ShelfRenderCandidate {
    shelf_state: ShelfState,
    render_sequence: u64,
}

impl ShelfRenderCandidate {
    pub(crate) fn rendered(self, rendered_monotonic_ns: Option<u64>, render_order: ShelfRenderOrder) -> ShelfRenderSample {
        ShelfRenderSample { shelf_state: self.shelf_state, render_sequence: self.render_sequence,
            render_order, rendered_monotonic_ns, submission: None }
    }
}

impl ShelfRenderSample {
    pub(crate) fn observe_submission(&mut self, framebuffer: &Framebuffer, submission: RefreshSubmission,
        full_window: bool)
    {
        self.observe_submission_with(framebuffer, submission, full_window, monotonic_ns);
    }

    fn observe_submission_with(&mut self, framebuffer: &Framebuffer, submission: RefreshSubmission,
        full_window: bool, clock: impl FnOnce() -> Option<u64>)
    {
        if full_window && submission.accepted && !submission.dithering_delivery_uncertain
            && let Some(update_marker) = submission.update_marker.filter(|marker| *marker != 0
                && Some(*marker) == framebuffer.last_accepted_update_marker())
            && let Some(submitted_monotonic_ns) = clock()
            && self.rendered_monotonic_ns.is_none_or(|rendered| rendered <= submitted_monotonic_ns)
        {
            self.submission = Some(ShelfRenderSubmission { update_marker, submitted_monotonic_ns,
                completed_monotonic_ns: None });
        }
    }

    pub(crate) fn observe_driver_completion(&mut self, framebuffer: &Framebuffer) {
        self.observe_driver_completion_with(framebuffer, monotonic_ns);
    }

    fn observe_driver_completion_with(&mut self, framebuffer: &Framebuffer, clock: impl FnOnce() -> Option<u64>) {
        if let Some(submission) = self.submission.as_mut()
            && framebuffer.last_completed_update_marker() == Some(submission.update_marker)
        {
            submission.completed_monotonic_ns = clock().filter(|completed| *completed >= submission.submitted_monotonic_ns);
        }
    }
}

pub(crate) fn covers_window(rect: Option<PhysicalRect>, width: u32, height: u32) -> bool {
    width > 0 && height > 0 && rect == Some(PhysicalRect { x: 0, y: 0, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn only_a_complete_unobscured_shelf_can_be_observed_once() {
        for state in [ShelfState::Ready, ShelfState::Empty, ShelfState::SignedOut] {
            let scene = Rc::new(Cell::new(ShelfRenderScene::Booting));
            let current = scene.clone();
            let output = Rc::new(Cell::new(None));
            let sink = output.clone();
            let trace = ShelfRenderTrace::new(move || current.get(), move |sample| sink.set(Some(sample)));
            assert!(trace.candidate().is_none());
            scene.set(ShelfRenderScene::Blocked);
            assert!(trace.candidate().is_none());
            scene.set(ShelfRenderScene::Shelf(state));
            let candidate = trace.candidate().unwrap();
            let sample = candidate.rendered(Some(50), ShelfRenderOrder::PrivateRenderFirst);
            assert_eq!(sample.shelf_state, state);
            assert_eq!(sample.render_order, ShelfRenderOrder::PrivateRenderFirst);
            assert_eq!(sample.submission, None);
            trace.emit(sample);
            assert_eq!(output.get(), Some(sample));
            assert!(trace.candidate().is_none());
        }
    }

    #[test]
    fn a_non_shelf_startup_or_interrupted_frame_is_not_relabelled() {
        let scene = Rc::new(Cell::new(ShelfRenderScene::Other));
        let current = scene.clone();
        let trace = ShelfRenderTrace::new(move || current.get(), |_| panic!("the startup destination was not Shelf"));
        assert!(trace.candidate().is_none());
        scene.set(ShelfRenderScene::Shelf(ShelfState::Ready));
        assert!(trace.candidate().is_none());

        let trace = ShelfRenderTrace::new(|| ShelfRenderScene::Shelf(ShelfState::Empty), |_| panic!("the trace was retired"));
        let candidate = trace.candidate().unwrap();
        let sample = candidate.rendered(Some(50), ShelfRenderOrder::WaitFirst);
        trace.retire();
        trace.emit(sample);
    }

    #[test]
    fn submission_and_driver_completion_cannot_claim_a_preceding_or_partial_update() {
        let trace = ShelfRenderTrace::new(|| ShelfRenderScene::Shelf(ShelfState::Ready), |_| {});
        let candidate = trace.candidate().unwrap();
        let framebuffer = Framebuffer::memory_for_test(3, 2, 3, 1, 0).unwrap();
        let preceding = framebuffer.fake_accept_update_for_test();
        framebuffer.fake_complete_latest_update_for_test().unwrap();
        let current = framebuffer.fake_accept_update_for_test();
        let rejected = RefreshSubmission { accepted: false, ..current };
        let no_clock = || panic!("a preceding, rejected or partial submission is not timed");
        let mut sample = candidate.rendered(Some(50), ShelfRenderOrder::WaitFirst);
        sample.observe_submission_with(&framebuffer, preceding, true, no_clock);
        sample.observe_submission_with(&framebuffer, rejected, true, no_clock);
        sample.observe_submission_with(&framebuffer, current, false, no_clock);
        assert_eq!(sample.submission, None);
        assert!(!covers_window(Some(PhysicalRect { x: 0, y: 0, width: 1, height: 2 }), 3, 2));
        assert!(covers_window(Some(PhysicalRect { x: 0, y: 0, width: 3, height: 2 }), 3, 2));

        sample.observe_submission_with(&framebuffer, current, true, || Some(90));
        sample.observe_driver_completion_with(&framebuffer, no_clock);
        assert_eq!(sample.rendered_monotonic_ns, Some(50));
        assert_eq!(sample.submission.unwrap().submitted_monotonic_ns, 90);
        assert_eq!(sample.submission.unwrap().completed_monotonic_ns, None);
        framebuffer.fake_complete_latest_update_for_test().unwrap();
        sample.observe_driver_completion_with(&framebuffer, || Some(120));
        assert_eq!(sample.submission.unwrap().completed_monotonic_ns, Some(120));

        let mut fallback = candidate.rendered(None, ShelfRenderOrder::InitialFill);
        fallback.observe_submission_with(&framebuffer, current, true, || Some(90));
        assert_eq!(fallback.rendered_monotonic_ns, None);
        assert_eq!(fallback.submission.unwrap().submitted_monotonic_ns, 90);
    }
}
