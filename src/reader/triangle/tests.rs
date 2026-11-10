//! Pixel equivalence, bounded planning and cooperative abandonment.

use std::{cell::Cell, hint::black_box, sync::atomic::{AtomicBool, Ordering}};

use image::{GrayImage, Luma, imageops::FilterType};

use super::*;

fn reference_byte(value: f32) -> u8 {
    let clamped = if value < 0.0 { 0.0 } else if value > 255.0 { 255.0 } else { value };
    clamped.round() as u8
}

fn check_byte(bits: u32) {
    let value = f32::from_bits(bits);
    assert_eq!(nearest_u8(value), reference_byte(value), "Input bits: {bits:08x}");
}

#[test]
fn exact_rounding_matches_halfway_neighbors_and_u8_edges() {
    for integer in 0..=255 {
        let halfway = (integer as f32 + 0.5).to_bits();
        for offset in -2i64..=2 {
            let bits = (i64::from(halfway) + offset) as u32;
            check_byte(bits);
            check_byte(bits | 0x8000_0000);
        }
        let exact = (integer as f32).to_bits();
        for offset in -1i64..=1 {
            if let Ok(bits) = u32::try_from(i64::from(exact) + offset) { check_byte(bits); }
        }
    }
    // Floating addition rounds this midpoint upward before the integer cast.
    let below_half = f32::from_bits(0x3eff_ffff);
    assert_eq!(nearest_u8(below_half), 0);
    assert_eq!((below_half + 0.5) as u8, 1);
}

#[test]
fn exact_rounding_preserves_special_values_and_exponent_boundaries() {
    for bits in [0, 1, 0x007f_ffff, 0x0080_0000, 0x437e_7fff, 0x437e_8000,
        0x437e_8001, 0x437e_ffff, 0x437f_0000, 0x437f_0001, 0x7f7f_ffff,
        0x7f80_0000, 0x7f80_0001, 0x7fbf_ffff, 0x7fc0_0000, 0x7fff_ffff,
        0x8000_0000, 0x8000_0001, 0xff7f_ffff, 0xff80_0000, 0xff80_0001,
        0xffbf_ffff, 0xffc0_0000, 0xffff_ffff]
    {
        check_byte(bits);
    }
    for exponent in 1u32..=254 {
        let bits = exponent << 23;
        for neighbor in [bits - 1, bits, bits + 1] {
            check_byte(neighbor);
            check_byte(neighbor | 0x8000_0000);
        }
    }
}

#[test]
#[ignore = "Run explicitly in release mode on the designated remote worker"]
fn exhaustive_rounding_core_matches_the_original_float_conversion() {
    assert!(!cfg!(debug_assertions), "Use a release build for the complete f32 core scan");
    // The classification branches handle all other bit patterns directly.
    // This interval contains every 75,431,936 finite f32 value in [0.5, 255).
    for bits in 0x3f00_0000u32..0x437f_0000u32 {
        let value = black_box(f32::from_bits(bits));
        assert_eq!(nearest_u8(value), reference_byte(value), "Input bits: {bits:08x}");
    }
}

pub(super) fn source(width: u32, height: u32, mut random: u64) -> GrayImage {
    GrayImage::from_fn(width, height, |_, _| {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        Luma([(random >> 16) as u8])
    })
}

pub(super) fn assert_image(actual: &GrayImage, expected: &GrayImage) {
    assert_eq!(actual.dimensions(), expected.dimensions());
    assert_eq!(actual.color_space(), expected.color_space());
    assert_eq!(actual.as_raw(), expected.as_raw());
}

/// Preserve the complete pre-streaming 0.1.21 dataflow independently of the
/// current row writer. The pinned generic implementation remains the numeric
/// oracle; this reference also detects changes against the prior matrix path.
pub(super) fn full_matrix_reference(source: &GrayImage, crop: SourceRect, width: u32, height: u32) -> GrayImage {
    let view = image::imageops::crop_imm(source, crop.x, crop.y, crop.width, crop.height);
    let (source_width, source_height) = view.dimensions();
    if source_width == 0 || source_height == 0 || width == 0 || height == 0
        || (source_width, source_height) == (width, height)
    {
        return image::imageops::resize(&*view, width, height, FilterType::Triangle);
    }
    let source_stride = source.width() as usize;
    let intermediate_stride = source_width as usize;
    let intermediate_len = intermediate_stride.checked_mul(height as usize).unwrap();
    let mut intermediate = vec![0.0f32; intermediate_len];
    let source_pixels = source.as_raw();
    let crop_x = crop.x as usize;
    let crop_y = crop.y as usize;
    let vertical = Axis::new(source_height, height);
    let mut weights = Vec::new();
    for output_y in 0..height {
        let first = vertical.weights(output_y, &mut weights) as usize;
        let output_row = output_y as usize * intermediate_stride;
        for x in 0..source_width {
            let mut total = 0.0f32;
            for (offset, weight) in weights.iter().enumerate() {
                let source_index = (crop_y + first + offset) * source_stride + crop_x + x as usize;
                let value = source_pixels[source_index] as f32;
                total += value * weight;
            }
            intermediate[output_row + x as usize] = total;
        }
    }
    drop(weights);
    let horizontal = Axis::new(source_width, width);
    let plan = match HorizontalPlan::build(&horizontal, width, HORIZONTAL_PLAN_BYTES, None) {
        PlanBuild::Ready(plan) => Some(plan),
        PlanBuild::Fallback => None,
        PlanBuild::Cancelled => unreachable!("The matrix reference has no cancellation source"),
    };
    let mut output = view.buffer_with_dimensions(width, height);
    let output_pixels: &mut [u8] = output.as_mut();
    let output_stride = width as usize;
    if let Some(plan) = plan {
        for y in 0..height as usize {
            let input_row = y * intermediate_stride;
            let output_row = y * output_stride;
            for (x, column) in plan.columns.iter().enumerate() {
                let weights = plan.weights(column);
                let total = accumulate_weights(0.0, weights, |offset| intermediate[input_row + column.first + offset]);
                output_pixels[output_row + x] = nearest_u8(total);
            }
        }
    } else {
        let mut weights = Vec::new();
        for output_x in 0..width {
            let first = horizontal.weights(output_x, &mut weights) as usize;
            for y in 0..height as usize {
                let input_row = y * intermediate_stride;
                let total = accumulate_weights(0.0, &weights, |offset| intermediate[input_row + first + offset]);
                output_pixels[y * output_stride + output_x as usize] = nearest_u8(total);
            }
        }
    }
    output
}

pub(super) fn compare_paths(source: &GrayImage, crop: SourceRect, width: u32, height: u32) {
    let view = image::imageops::crop_imm(source, crop.x, crop.y, crop.width, crop.height);
    let expected = image::imageops::resize(&*view, width, height, FilterType::Triangle);
    let matrix = full_matrix_reference(source, crop, width, height);
    assert_image(&matrix, &expected);
    for budget in [HORIZONTAL_PLAN_BYTES, 0] {
        let normal = resize_with_plan_budget(source, crop, width, height, budget);
        assert_image(&normal, &expected);
        assert_image(&normal, &matrix);
        let flag = AtomicBool::new(false);
        let checks = Cell::new(0usize);
        let probe = |_: RenderWork, _: usize| checks.set(checks.get() + 1);
        let cancellation = RenderCancellation::with_probe(&flag, &probe);
        let cancellable = resize_cancellable_with_plan_budget(source, crop, width, height, &cancellation, budget)
            .expect("A false cancellation flag must return the complete image");
        assert!(checks.get() > 0, "Some(false) must exercise the real cancellable path");
        assert_image(&cancellable, &normal);
        assert_image(&cancellable, &expected);
        assert_image(&cancellable, &matrix);
    }
}

#[test]
fn planned_fallback_and_uncancelled_paths_match_random_crops_and_metadata() {
    let mut random = 0x7d91_a603_c875_eb42u64;
    for case in 0..64u64 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let width = (random as u32 % 97) + 1;
        let height = ((random >> 32) as u32 % 127) + 1;
        let mut pixels = source(width, height, random);
        let color_space = if case % 2 == 0 { image::metadata::Cicp::DISPLAY_P3 }
            else { image::metadata::Cicp::SRGB_LINEAR };
        pixels.set_color_space(color_space).unwrap();
        let x = case as u32 % width;
        let y = case as u32 % height;
        let crop = SourceRect { x, y, width: width - x, height: height - y };
        compare_paths(&pixels, crop, (random as u32 % 131) + 1, ((random >> 32) as u32 % 149) + 1);
    }
}

#[test]
fn planned_fallback_and_uncancelled_paths_match_extreme_ratios_and_copy_cases() {
    for (width, height) in [(0, 0), (0, 7), (9, 0), (1, 4093), (4093, 1),
        (3, 4093), (4093, 3), (1023, 17), (17, 1023), (29, 41)]
    {
        let mut pixels = source(width, height, 0x4093_7311);
        pixels.set_color_space(image::metadata::Cicp::DISPLAY_P3).unwrap();
        for (target_width, target_height) in [(0, 0), (0, 7), (9, 0), (width, height), (1, 1), (17, 31)] {
            compare_paths(&pixels, SourceRect::whole(width, height), target_width, target_height);
        }
    }
}

#[test]
fn plan_budget_accounts_for_both_actual_vector_capacities() {
    let axis = Axis::new(1700, 1786);
    let PlanBuild::Ready(plan) = HorizontalPlan::build(&axis, 1786, HORIZONTAL_PLAN_BYTES, None) else {
        panic!("The ordinary full-page plan must fit its fixed budget");
    };
    let actual_bytes = plan_bytes(plan.columns.capacity(), plan.coefficients.capacity()).unwrap();
    assert!(actual_bytes <= HORIZONTAL_PLAN_BYTES);
    assert_eq!(plan.columns.len(), 1786);
    assert!(plan.coefficients.capacity() >= plan.coefficients.len());
    let mut expected = Vec::new();
    for (x, column) in plan.columns.iter().enumerate() {
        assert_eq!(column.first, axis.weights(x as u32, &mut expected) as usize);
        assert_eq!(plan.weights(column), expected.as_slice());
    }
    assert!(matches!(HorizontalPlan::build(&axis, 1786, 0, None), PlanBuild::Fallback));
}

#[test]
fn huge_plan_requests_fall_back_without_requiring_huge_pixel_buffers() {
    assert!(plan_bytes(usize::MAX, 1).is_none());
    assert!(plan_bytes(1, usize::MAX).is_none());
    assert!(matches!(HorizontalPlan::build(&Axis::new(1, u32::MAX), u32::MAX,
        HORIZONTAL_PLAN_BYTES, None), PlanBuild::Fallback));
    // Metadata alone fits, but the complete column coefficients do not.
    assert!(matches!(HorizontalPlan::build(&Axis::new(1_000_000, 1), 1,
        HORIZONTAL_PLAN_BYTES, None), PlanBuild::Fallback));
}

fn cancel_plan_pass(fill: bool) {
    let pixels = source(17, 29, 0x1729);
    let flag = AtomicBool::new(false);
    let starts = Cell::new(0usize);
    let stopped_at = Cell::new(0usize);
    let expected_pass = if fill { 2 } else { 1 };
    let probe = |work, completed| {
        if work != RenderWork::ResizePlan { return; }
        if completed == 0 { starts.set(starts.get() + 1); }
        if starts.get() == expected_pass && completed == WEIGHT_BLOCK {
            stopped_at.set(completed);
            flag.store(true, Ordering::Release);
        }
    };
    let cancellation = RenderCancellation::with_probe(&flag, &probe);
    let result = resize_cancellable(&pixels, SourceRect::whole(17, 29), 1025, 31, &cancellation);
    assert!(result.is_none(), "A cancelled plan must not fall through to column rendering");
    assert_eq!(stopped_at.get(), WEIGHT_BLOCK);
    assert_eq!(starts.get(), expected_pass);
}

#[test]
fn cancellation_during_plan_counting_returns_no_image() { cancel_plan_pass(false); }

#[test]
fn cancellation_during_plan_filling_returns_no_image() { cancel_plan_pass(true); }

fn cancel_long_planned_column(normalization: bool) {
    let pixels = source(4097, 29, 0x4097);
    let flag = AtomicBool::new(false);
    let starts = Cell::new(0usize);
    let coefficient_passes = Cell::new(0usize);
    let stopped_at = Cell::new(0usize);
    let expected_pass = if normalization { 2 } else { 1 };
    let probe = |work, completed| {
        if work == RenderWork::ResizePlan && completed == 0 { starts.set(starts.get() + 1); }
        if starts.get() == 2 && work == RenderWork::ResizeWeights {
            if completed == 0 { coefficient_passes.set(coefficient_passes.get() + 1); }
            if coefficient_passes.get() == expected_pass && completed == WEIGHT_BLOCK {
                stopped_at.set(completed);
                flag.store(true, Ordering::Release);
            }
        }
    };
    let cancellation = RenderCancellation::with_probe(&flag, &probe);
    // One column prevents the next column's generation from masquerading as
    // this column's normalization pass if a checkpoint were removed.
    assert!(resize_cancellable(&pixels, SourceRect::whole(4097, 29), 1, 47, &cancellation).is_none());
    assert_eq!(stopped_at.get(), WEIGHT_BLOCK);
    assert_eq!(coefficient_passes.get(), expected_pass);
}

#[test]
fn cancellation_inside_planned_coefficient_generation_returns_no_image() { cancel_long_planned_column(false); }

#[test]
fn cancellation_inside_planned_coefficient_normalization_returns_no_image() { cancel_long_planned_column(true); }

#[test]
fn cancellation_during_row_or_fallback_pixels_never_returns_a_partial_image() {
    let pixels = source(361, 517, 0x361517);
    for budget in [HORIZONTAL_PLAN_BYTES, 0] {
        let flag = AtomicBool::new(false);
        let stopped_at = Cell::new(0usize);
        let probe = |work, completed| {
            if work == RenderWork::ResizeHorizontal && completed == RENDER_PIXEL_BLOCK {
                stopped_at.set(completed);
                flag.store(true, Ordering::Release);
            }
        };
        let cancellation = RenderCancellation::with_probe(&flag, &probe);
        assert!(resize_cancellable_with_plan_budget(&pixels, SourceRect::whole(361, 517),
            400, 600, &cancellation, budget).is_none());
        assert_eq!(stopped_at.get(), RENDER_PIXEL_BLOCK);
    }
}

#[test]
fn cancellation_inside_long_row_or_fallback_accumulation_returns_no_image() {
    let pixels = source(4097, 29, 0x4097_29);
    for budget in [HORIZONTAL_PLAN_BYTES, 0] {
        let flag = AtomicBool::new(false);
        let first_pixel_checks = Cell::new(0usize);
        let probe = |work, completed| {
            if work == RenderWork::ResizeHorizontal && completed == 0 {
                first_pixel_checks.set(first_pixel_checks.get() + 1);
                // Outer pixel block, first coefficient block, then the second
                // coefficient block after 64 actual products in the same pixel.
                if first_pixel_checks.get() == 3 { flag.store(true, Ordering::Release); }
            }
        };
        let cancellation = RenderCancellation::with_probe(&flag, &probe);
        assert!(resize_cancellable_with_plan_budget(&pixels, SourceRect::whole(4097, 29),
            1, 47, &cancellation, budget).is_none());
        assert_eq!(first_pixel_checks.get(), 3);
    }
}

#[test]
fn already_cancelled_plan_does_not_degrade_to_budget_fallback() {
    let flag = AtomicBool::new(true);
    let cancellation = RenderCancellation::for_flag(&flag);
    assert!(matches!(HorizontalPlan::build(&Axis::new(17, 31), 31, 0, Some(&cancellation)),
        PlanBuild::Cancelled));
}

#[test]
fn disabled_cancellation_uses_default_path_without_checkpoint_calls() {
    let pixels = source(73, 91, 0x7391);
    let probe = |_: RenderWork, _: usize| panic!("The disabled path must not execute cancellation checkpoints");
    let cancellation = RenderCancellation { flag: None, probe: Some(&probe) };
    let crop = SourceRect { x: 7, y: 11, width: 61, height: 73 };
    let expected = resize(&pixels, crop, 83, 109);
    let actual = resize_cancellable(&pixels, crop, 83, 109, &cancellation).unwrap();
    assert_image(&actual, &expected);
}
