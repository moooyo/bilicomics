//! Streaming storage, pre-streaming parity, and cancellation between rows.

use std::{cell::{Cell, RefCell}, sync::atomic::{AtomicBool, Ordering}};

use super::*;
use super::tests::{assert_image, compare_paths, full_matrix_reference, source};

#[derive(Default)]
struct IntermediateObservation {
    fail_row_reservation: bool,
    allocations: Vec<(usize, usize)>,
}

thread_local! {
    static INTERMEDIATES: RefCell<Option<IntermediateObservation>> = const { RefCell::new(None) };
}

pub(super) fn row_reservation_fails() -> bool {
    INTERMEDIATES.with(|state| state.borrow().as_ref().is_some_and(|value| value.fail_row_reservation))
}

pub(super) fn record_intermediate_allocation(buffer: &Vec<f32>) {
    INTERMEDIATES.with(|state| {
        if let Some(observation) = state.borrow_mut().as_mut() {
            // Observe the vector that was actually allocated and initialized,
            // rather than a prediction derived from the requested dimensions.
            observation.allocations.push((buffer.len(), buffer.capacity()));
        }
    });
}

struct ObservationGuard;

impl Drop for ObservationGuard {
    fn drop(&mut self) {
        INTERMEDIATES.with(|state| { state.borrow_mut().take(); });
    }
}

fn observe_intermediates<T>(fail_row_reservation: bool, run: impl FnOnce() -> T) -> (T, Vec<(usize, usize)>) {
    INTERMEDIATES.with(|state| {
        assert!(state.borrow().is_none(), "Allocation observations must not nest");
        *state.borrow_mut() = Some(IntermediateObservation { fail_row_reservation, ..Default::default() });
    });
    let _guard = ObservationGuard;
    let result = run();
    let allocations = INTERMEDIATES.with(|state| state.borrow_mut().take().unwrap().allocations);
    (result, allocations)
}

#[test]
fn successful_plans_allocate_one_actual_f32_row_independent_of_target_height() {
    let mut pixels = source(1709, 41, 0x1700_1786);
    pixels.set_color_space(image::metadata::Cicp::DISPLAY_P3).unwrap();
    let crop = SourceRect { x: 9, y: 7, width: 1700, height: 31 };
    for height in [47, 149] {
        let expected = full_matrix_reference(&pixels, crop, 1786, height);
        for cancellable in [false, true] {
            let flag = AtomicBool::new(false);
            let cancellation = RenderCancellation::for_flag(&flag);
            let (actual, allocations) = observe_intermediates(false, || {
                if cancellable {
                    resize_cancellable(&pixels, crop, 1786, height, &cancellation).unwrap()
                } else {
                    resize(&pixels, crop, 1786, height)
                }
            });
            assert_image(&actual, &expected);
            assert_eq!(allocations.len(), 1);
            let (length, capacity) = allocations[0];
            assert_eq!(length, crop.width as usize);
            assert!(capacity >= length);
            assert!(capacity < crop.width as usize * height as usize,
                "A planned resize must not allocate the old full-height f32 matrix");
        }
    }
}

#[test]
fn plan_and_row_reservation_failures_keep_the_complete_matrix_fallback() {
    let mut pixels = source(83, 109, 0x83109);
    pixels.set_color_space(image::metadata::Cicp::SRGB_LINEAR).unwrap();
    let crop = SourceRect { x: 7, y: 11, width: 73, height: 91 };
    let expected = full_matrix_reference(&pixels, crop, 97, 127);
    for (budget, fail_row_reservation) in [(0, false), (HORIZONTAL_PLAN_BYTES, true)] {
        for cancellable in [false, true] {
            let flag = AtomicBool::new(false);
            let cancellation = RenderCancellation::for_flag(&flag);
            let (actual, allocations) = observe_intermediates(fail_row_reservation, || {
                if cancellable {
                    resize_cancellable_with_plan_budget(&pixels, crop, 97, 127, &cancellation, budget).unwrap()
                } else {
                    resize_with_plan_budget(&pixels, crop, 97, 127, budget)
                }
            });
            assert_image(&actual, &expected);
            assert_eq!(allocations.len(), 1);
            let (length, capacity) = allocations[0];
            assert_eq!(length, crop.width as usize * 127);
            assert!(capacity >= length);
        }
    }
}

#[test]
fn a_naturally_oversized_plan_keeps_exact_pixels_and_the_matrix_fallback() {
    let mut pixels = source(70_001, 3, 0x70001_3);
    pixels.set_color_space(image::metadata::Cicp::DISPLAY_P3).unwrap();
    let crop = SourceRect::whole(pixels.width(), pixels.height());
    assert!(matches!(HorizontalPlan::build(&Axis::new(pixels.width(), 1), 1,
        HORIZONTAL_PLAN_BYTES, None), PlanBuild::Fallback));
    let expected = image::imageops::resize(&pixels, 1, 7, FilterType::Triangle);
    let matrix = full_matrix_reference(&pixels, crop, 1, 7);
    assert_image(&matrix, &expected);
    for cancellable in [false, true] {
        let flag = AtomicBool::new(false);
        let cancellation = RenderCancellation::for_flag(&flag);
        let (actual, allocations) = observe_intermediates(false, || {
            if cancellable {
                resize_cancellable(&pixels, crop, 1, 7, &cancellation).unwrap()
            } else {
                resize(&pixels, crop, 1, 7)
            }
        });
        assert_image(&actual, &expected);
        assert_image(&actual, &matrix);
        assert_eq!(allocations.len(), 1);
        assert_eq!(allocations[0].0, pixels.width() as usize * 7);
        assert!(allocations[0].1 >= allocations[0].0);
    }
}

#[test]
fn streaming_and_matrix_paths_match_clamped_crops_and_empty_origins() {
    let mut pixels = source(37, 53, 0x3753);
    pixels.set_color_space(image::metadata::Cicp::DISPLAY_P3).unwrap();
    for crop in [
        SourceRect { x: 11, y: 7, width: u32::MAX, height: u32::MAX },
        SourceRect { x: 37, y: 0, width: 17, height: 29 },
        SourceRect { x: 0, y: 53, width: 17, height: 29 },
        SourceRect { x: u32::MAX, y: u32::MAX, width: 17, height: 29 },
    ] {
        for (width, height) in [(1, 1), (26, 46), (71, 83), (0, 7)] {
            compare_paths(&pixels, crop, width, height);
        }
    }
}

#[test]
fn cancelling_plan_construction_allocates_no_vertical_pixel_buffer() {
    let pixels = source(17, 29, 0x1729);
    let flag = AtomicBool::new(false);
    let pixel_checks = Cell::new(0usize);
    let probe = |work, completed| {
        if work == RenderWork::ResizePlan && completed == WEIGHT_BLOCK {
            flag.store(true, Ordering::Release);
        }
        if matches!(work, RenderWork::ResizeVertical | RenderWork::ResizeHorizontal) {
            pixel_checks.set(pixel_checks.get() + 1);
        }
    };
    let cancellation = RenderCancellation::with_probe(&flag, &probe);
    let (result, allocations) = observe_intermediates(false,
        || resize_cancellable(&pixels, SourceRect::whole(17, 29), 1025, 31, &cancellation));
    assert!(result.is_none());
    assert!(allocations.is_empty());
    assert_eq!(pixel_checks.get(), 0);
}

#[test]
fn planned_horizontal_cancellation_precedes_the_next_vertical_row() {
    let pixels = source(361, 517, 0x361517);
    let flag = AtomicBool::new(false);
    let vertical_checks = Cell::new(0usize);
    let last_vertical = Cell::new(0usize);
    let horizontal_stop = Cell::new(0usize);
    let probe = |work, completed| {
        if work == RenderWork::ResizeVertical {
            vertical_checks.set(vertical_checks.get() + 1);
            last_vertical.set(completed);
        }
        if work == RenderWork::ResizeHorizontal && completed == RENDER_PIXEL_BLOCK {
            horizontal_stop.set(completed);
            flag.store(true, Ordering::Release);
        }
    };
    let cancellation = RenderCancellation::with_probe(&flag, &probe);
    assert!(resize_cancellable(&pixels, SourceRect::whole(361, 517), 400, 600, &cancellation).is_none());
    assert_eq!(horizontal_stop.get(), RENDER_PIXEL_BLOCK);
    assert_eq!(vertical_checks.get(), 2);
    assert_eq!(last_vertical.get(), RENDER_PIXEL_BLOCK);
    assert!(last_vertical.get() < pixels.width() as usize);
}

#[test]
fn reused_row_storage_preserves_global_vertical_and_horizontal_pixel_counters() {
    let pixels = source(73, 91, 0x7391);
    for (target_work, expected_position) in [(RenderWork::ResizeVertical, 73), (RenderWork::ResizeHorizontal, 83)] {
        let flag = AtomicBool::new(false);
        let stopped_at = Cell::new(0usize);
        let probe = |work, completed| {
            if work == target_work && completed == expected_position {
                stopped_at.set(completed);
                flag.store(true, Ordering::Release);
            }
        };
        let cancellation = RenderCancellation::with_probe(&flag, &probe);
        assert!(resize_cancellable(&pixels, SourceRect::whole(73, 91), 83, 109, &cancellation).is_none());
        assert_eq!(stopped_at.get(), expected_position);
    }
}

#[test]
fn cancellation_inside_long_vertical_accumulation_returns_no_image() {
    let pixels = source(29, 4097, 0x29_4097);
    for budget in [HORIZONTAL_PLAN_BYTES, 0] {
        let flag = AtomicBool::new(false);
        let first_pixel_checks = Cell::new(0usize);
        let horizontal_checks = Cell::new(0usize);
        let probe = |work, completed| {
            if work == RenderWork::ResizeHorizontal { horizontal_checks.set(horizontal_checks.get() + 1); }
            if work == RenderWork::ResizeVertical && completed == 0 {
                first_pixel_checks.set(first_pixel_checks.get() + 1);
                // Outer pixel block, first coefficient block, and then the
                // next checkpoint after 64 actual products in this pixel.
                if first_pixel_checks.get() == 3 { flag.store(true, Ordering::Release); }
            }
        };
        let cancellation = RenderCancellation::with_probe(&flag, &probe);
        assert!(resize_cancellable_with_plan_budget(&pixels, SourceRect::whole(29, 4097),
            47, 17, &cancellation, budget).is_none());
        assert_eq!(first_pixel_checks.get(), 3);
        assert_eq!(horizontal_checks.get(), 0);
    }
}

#[test]
fn cancellation_at_the_streamed_output_boundary_returns_no_image() {
    let pixels = source(73, 91, 0x7391);
    let flag = AtomicBool::new(false);
    let horizontal_started = Cell::new(false);
    let probe = |work, _| {
        if work == RenderWork::ResizeHorizontal { horizontal_started.set(true); }
        if work == RenderWork::Boundary && horizontal_started.get() { flag.store(true, Ordering::Release); }
    };
    let cancellation = RenderCancellation::with_probe(&flag, &probe);
    assert!(resize_cancellable(&pixels, SourceRect::whole(73, 91), 83, 109, &cancellation).is_none());
    assert!(horizontal_started.get());
    assert!(flag.load(Ordering::Acquire));
}
