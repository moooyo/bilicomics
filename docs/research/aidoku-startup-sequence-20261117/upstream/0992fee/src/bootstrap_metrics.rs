//! Observe startup output without scheduling extra frames or blocking display work.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use aidoku_kindle::{
    platform::Platform,
    reader_timing,
    ui::{AppWindow, Commands, State},
};
use slint::ComponentHandle;

use crate::{
    bootstrap_lifecycle,
    bootstrap_startup::{ApplicationSlot, Startup},
};

pub fn observe(
    platform: &Rc<Platform>,
    ui: &AppWindow,
    startup: &Rc<RefCell<Startup>>,
    application: &ApplicationSlot,
    lifecycle: &Rc<RefCell<bootstrap_lifecycle::Coordinator>>,
    overlap: bool,
) {
    let ui = ui.as_weak();
    let startup = Rc::downgrade(startup);
    let application = Rc::downgrade(application);
    let lifecycle = Rc::downgrade(lifecycle);
    let observed_platform = Rc::downgrade(platform);
    let first_library = Cell::new(None::<u64>);
    let library_input_ready = Cell::new(false);
    let covers_settled = Cell::new(false);
    platform.set_checkpoint_observer(move |frame| {
        if !frame.active || !bootstrap_lifecycle::display_activity_allowed() {
            return;
        }
        let Some(lifecycle) = lifecycle.upgrade() else {
            return;
        };
        if !lifecycle.borrow().application_work_allowed() {
            return;
        }
        if frame.initial && frame.full && frame.submitted {
            reader_timing::startup_frame("startup_initial_frame_submitted", frame, None);
            if overlap && let Some(startup) = startup.upgrade() {
                startup.borrow_mut().prepare_after_submission();
            }
        }
        let Some(platform) = observed_platform.upgrade() else {
            return;
        };
        if !reader_timing::enabled() && !platform.startup_render_pending() {
            return;
        }
        if frame.initial && !frame.submitted && !frame.input_blocked {
            reader_timing::startup_frame("startup_initial_input_ready", frame, None);
        }
        let Some(ui) = ui.upgrade() else { return };
        if ui.global::<State>().get_booting() {
            return;
        }
        if frame.submitted && platform.release_startup_render_priority() {
            reader_timing::mark("startup_background_covers_admitted");
            if let Some(app) = application.upgrade().and_then(|slot| slot.borrow().clone()) {
                app.borrow_mut().begin_startup_cover_publication();
            }
            let ui = ui.as_weak();
            // Return to the regular lifecycle-gated dispatcher before starting work.
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                if let Some(ui) = ui.upgrade() {
                    ui.global::<Commands>().invoke_job_ready();
                }
            });
        }
        if !reader_timing::enabled() || (covers_settled.get() && library_input_ready.get()) {
            return;
        }
        let Some(app) = application.upgrade().and_then(|slot| slot.borrow().clone()) else {
            return;
        };
        let Some(covers) = app.borrow().startup_cover_projection() else {
            return;
        };
        if frame.submitted {
            if first_library.get().is_none() {
                first_library.set(Some(frame.sequence));
                reader_timing::startup_frame(
                    "startup_library_frame_submitted",
                    frame,
                    Some(covers),
                );
            }
            reader_timing::startup_frame("startup_cover_frame_submitted", frame, Some(covers));
            if !covers.2 && !covers_settled.replace(true) {
                reader_timing::startup_frame("startup_visible_covers_settled", frame, Some(covers));
            }
        } else if !frame.input_blocked
            && first_library
                .get()
                .is_some_and(|first| frame.sequence >= first)
            && !library_input_ready.replace(true)
        {
            reader_timing::startup_frame("startup_library_input_ready", frame, Some(covers));
        }
    });
}
