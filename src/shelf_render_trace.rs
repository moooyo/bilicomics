use bilicomics::{app::App, performance, ui::AppWindow};
use slint::ComponentHandle;
use slint_backend_kindle::{DisplayLifecycleHandle, ShelfRenderOrder, ShelfRenderSample, ShelfRenderScene, ShelfState};
use std::rc::Rc;

use crate::prepared_handoff::PreparedHandoff;

pub(crate) fn install(display: &DisplayLifecycleHandle, ui: &AppWindow, handoff: Option<Rc<PreparedHandoff>>) {
    let window = ui.as_weak();
    let scene = move || window.upgrade().map_or(ShelfRenderScene::Other, |ui| App::shelf_render_scene(&ui));
    if let Some(handoff) = handoff {
        display.observe_shelf_render(scene, move |sample| {
            handoff.qualify_shelf(sample.rendered_monotonic_ns);
            record(sample);
        });
    } else {
        display.observe_shelf_render(scene, record);
    }
}

fn record(sample: ShelfRenderSample) {
    let shelf_state = match sample.shelf_state {
        ShelfState::Ready => "ready",
        ShelfState::Empty => "empty",
        ShelfState::SignedOut => "signed_out",
    };
    if let Some(rendered_monotonic_ns) = sample.rendered_monotonic_ns {
        let render_order = match sample.render_order {
            ShelfRenderOrder::InitialFill => "initial_fill",
            ShelfRenderOrder::WaitFirst => "wait_then_render",
            ShelfRenderOrder::PrivateRenderFirst => "render_then_wait",
        };
        performance::shelf_render("shelf_rendered", &serde_json::json!({
            "shelf_state": shelf_state,
            "render_sequence": sample.render_sequence,
            "render_order": render_order,
            "rendered_monotonic_ns": rendered_monotonic_ns,
        }));
    }
    let Some(submission) = sample.submission else { return; };
    performance::shelf_render("shelf_submitted", &serde_json::json!({
        "shelf_state": shelf_state,
        "render_sequence": sample.render_sequence,
        "update_marker": submission.update_marker,
        "submitted_monotonic_ns": submission.submitted_monotonic_ns,
    }));
    if let Some(completed_monotonic_ns) = submission.completed_monotonic_ns {
        performance::shelf_render("shelf_driver_completed", &serde_json::json!({
            "shelf_state": shelf_state,
            "render_sequence": sample.render_sequence,
            "update_marker": submission.update_marker,
            "completed_monotonic_ns": completed_monotonic_ns,
        }));
    }
}
