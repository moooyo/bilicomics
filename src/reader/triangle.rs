//! Single-channel specialization of image 0.25.10's separable Triangle resize.
//!
//! The sampling algorithm is adapted from image under MIT OR Apache-2.0.
//! The pinned reference sources, complete upstream licenses, and derivation
//! notes are retained in `vendor/image-triangle-reference-0.25.10/`. Keep the
//! vertical-first order and each f32 operation in the reference order; changing
//! precision, summation order, or rounding can change the final reader pixels.

use std::mem::size_of;

use image::{GenericImageView, GrayImage, imageops::FilterType};

use super::{RENDER_COPY_BLOCK_BYTES, RENDER_PIXEL_BLOCK, RenderCancellation, RenderWork, SourceRect};

const WEIGHT_BLOCK: usize = 64;
// Element capacities of the two retained plan vectors, not allocator overhead
// or the source, vertical row/fallback matrix and output pixel buffers.
const HORIZONTAL_PLAN_BYTES: usize = 256 * 1024;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod streaming_tests;

/// Resize the same immutable crop used by the generic reader path.
///
/// A planned resize retains one row of the reference's first f32 channel. The
/// other channels never contribute to its final Luma<u8> output. A failed plan
/// or row reservation retains the original full single-channel intermediate.
pub(super) fn resize(source: &GrayImage, crop: SourceRect, width: u32, height: u32) -> GrayImage {
    resize_with_plan_budget(source, crop, width, height, HORIZONTAL_PLAN_BYTES)
}

fn resize_with_plan_budget(source: &GrayImage, crop: SourceRect, width: u32, height: u32,
    plan_budget: usize) -> GrayImage
{
    let view = image::imageops::crop_imm(source, crop.x, crop.y, crop.width, crop.height);
    let (source_width, source_height) = view.dimensions();
    if source_width == 0 || source_height == 0 || width == 0 || height == 0
        || (source_width, source_height) == (width, height)
    {
        // Preserve the pinned implementation's empty-image and exact-copy
        // behavior, including the crop view's inherited color metadata.
        return image::imageops::resize(&*view, width, height, FilterType::Triangle);
    }

    let source_stride = source.width() as usize;
    let intermediate_stride = source_width as usize;
    let source_pixels = source.as_raw();
    let crop_x = crop.x as usize;
    let crop_y = crop.y as usize;
    let vertical = Axis::new(source_height, height);
    let horizontal = Axis::new(source_width, width);
    let plan = match HorizontalPlan::build(&horizontal, width, plan_budget, None) {
        PlanBuild::Ready(plan) => Some(plan),
        PlanBuild::Fallback => None,
        PlanBuild::Cancelled => unreachable!("The default resize has no cancellation source"),
    };
    let output_stride = width as usize;
    if let Some(plan) = plan {
        if let Some(mut row) = try_row_buffer(intermediate_stride) {
            // SubImage delegates buffer metadata to the underlying image, just
            // as it does for the reference's metadata-bearing intermediate.
            let mut output = view.buffer_with_dimensions(width, height);
            let output_pixels: &mut [u8] = output.as_mut();
            let mut weights = Vec::new();
            for output_y in 0..height {
                let first = vertical.weights(output_y, &mut weights) as usize;
                let source_start = (crop_y + first) * source_stride + crop_x;
                write_vertical_row(source_pixels, source_stride, source_start, &weights, &mut row);
                let output_row = output_y as usize * output_stride;
                for (x, column) in plan.columns.iter().enumerate() {
                    let weights = plan.weights(column);
                    let total = accumulate_weights(0.0, weights, |offset| row[column.first + offset]);
                    output_pixels[output_row + x] = nearest_u8(total);
                }
            }
            return output;
        }
        // Release the plan before the full-matrix fallback allocation.
    }

    // A plan budget/allocation failure or unavailable row reservation keeps
    // the original full intermediate and column traversal with identical pixels.
    let mut intermediate = full_intermediate(intermediate_stride, height);
    let mut weights = Vec::new();
    for output_y in 0..height {
        let first = vertical.weights(output_y, &mut weights) as usize;
        let source_start = (crop_y + first) * source_stride + crop_x;
        let output_row = output_y as usize * intermediate_stride;
        write_vertical_row(source_pixels, source_stride, source_start, &weights,
            &mut intermediate[output_row..output_row + intermediate_stride]);
    }
    drop(weights);
    let mut output = view.buffer_with_dimensions(width, height);
    let output_pixels: &mut [u8] = output.as_mut();
    let mut weights = Vec::new();
    for output_x in 0..width {
        let first = horizontal.weights(output_x, &mut weights) as usize;
        for y in 0..height as usize {
            let input_row = y * intermediate_stride;
            let total = accumulate_weights(0.0, &weights, |offset| intermediate[input_row + first + offset]);
            output_pixels[y * output_stride + output_x as usize] = nearest_u8(total);
        }
    }
    output
}

/// Application workers may abandon obsolete work between bounded CPU blocks.
/// A disabled cancellation source uses the default path without atomic loads.
/// Both paths share the plan and coefficient arithmetic. Checks do not reorder
/// any pixel's floating-point accumulation or publish a partial image.
pub(super) fn resize_cancellable(source: &GrayImage, crop: SourceRect, width: u32, height: u32,
    cancellation: &RenderCancellation<'_>) -> Option<GrayImage>
{
    resize_cancellable_with_plan_budget(source, crop, width, height, cancellation, HORIZONTAL_PLAN_BYTES)
}

fn resize_cancellable_with_plan_budget(source: &GrayImage, crop: SourceRect, width: u32, height: u32,
    cancellation: &RenderCancellation<'_>, plan_budget: usize) -> Option<GrayImage>
{
    if !cancellation.is_enabled() { return Some(resize_with_plan_budget(source, crop, width, height, plan_budget)); }
    if cancellation.is_cancelled(RenderWork::Boundary, 0) { return None; }
    let view = image::imageops::crop_imm(source, crop.x, crop.y, crop.width, crop.height);
    let (source_width, source_height) = view.dimensions();
    if source_width == 0 || source_height == 0 || width == 0 || height == 0 {
        // Empty-image behavior and metadata remain owned by the pinned helper.
        let output = image::imageops::resize(&*view, width, height, FilterType::Triangle);
        return (!cancellation.is_cancelled(RenderWork::Boundary, 0)).then_some(output);
    }
    if (source_width, source_height) == (width, height) {
        let mut output = view.buffer_with_dimensions(width, height);
        let output_pixels: &mut [u8] = output.as_mut();
        let pixels = source.as_raw();
        let source_stride = source.width() as usize;
        let width = width as usize;
        for y in 0..height as usize {
            let input_start = (crop.y as usize + y) * source_stride + crop.x as usize;
            let input = &pixels[input_start..input_start + width];
            let output = &mut output_pixels[y * width..(y + 1) * width];
            for first in (0..width).step_by(RENDER_COPY_BLOCK_BYTES) {
                if cancellation.is_cancelled(RenderWork::ResizeCopy, y * width + first) { return None; }
                let end = (first + RENDER_COPY_BLOCK_BYTES).min(width);
                output[first..end].copy_from_slice(&input[first..end]);
            }
        }
        return (!cancellation.is_cancelled(RenderWork::Boundary, 0)).then_some(output);
    }

    let source_stride = source.width() as usize;
    let intermediate_stride = source_width as usize;
    let source_pixels = source.as_raw();
    let crop_x = crop.x as usize;
    let crop_y = crop.y as usize;
    let vertical = Axis::new(source_height, height);
    let horizontal = Axis::new(source_width, width);
    let plan = match HorizontalPlan::build(&horizontal, width, plan_budget, Some(cancellation)) {
        PlanBuild::Ready(plan) => Some(plan),
        PlanBuild::Fallback => None,
        PlanBuild::Cancelled => return None,
    };
    if cancellation.is_cancelled(RenderWork::Boundary, 0) { return None; }
    let output_stride = width as usize;
    if let Some(plan) = plan {
        if let Some(mut row) = try_row_buffer(intermediate_stride) {
            let mut output = view.buffer_with_dimensions(width, height);
            let output_pixels: &mut [u8] = output.as_mut();
            let mut weights = Vec::new();
            for output_y in 0..height {
                let first = vertical.weights_cancellable(output_y, &mut weights, cancellation)? as usize;
                let source_start = (crop_y + first) * source_stride + crop_x;
                write_vertical_row_cancellable(source_pixels, source_stride, source_start, &weights,
                    &mut row, output_y as usize * intermediate_stride, cancellation)?;
                let output_row = output_y as usize * output_stride;
                for first_x in (0..output_stride).step_by(RENDER_PIXEL_BLOCK) {
                    // Counts logical output visits, even though row storage is reused.
                    if cancellation.is_cancelled(RenderWork::ResizeHorizontal, output_row + first_x) { return None; }
                    let end_x = (first_x + RENDER_PIXEL_BLOCK).min(output_stride);
                    for x in first_x..end_x {
                        let column = &plan.columns[x];
                        let weights = plan.weights(column);
                        let total = if weights.len() > WEIGHT_BLOCK {
                            accumulate_long_weights(weights, cancellation, RenderWork::ResizeHorizontal,
                                output_row + x, |offset| row[column.first + offset])?
                        } else {
                            accumulate_weights(0.0, weights, |offset| row[column.first + offset])
                        };
                        output_pixels[output_row + x] = nearest_u8(total);
                    }
                }
            }
            return (!cancellation.is_cancelled(RenderWork::Boundary, 0)).then_some(output);
        }
    }

    if cancellation.is_cancelled(RenderWork::Boundary, 0) { return None; }
    let mut intermediate = full_intermediate(intermediate_stride, height);
    let mut weights = Vec::new();
    for output_y in 0..height {
        let first = vertical.weights_cancellable(output_y, &mut weights, cancellation)? as usize;
        let source_start = (crop_y + first) * source_stride + crop_x;
        let output_row = output_y as usize * intermediate_stride;
        write_vertical_row_cancellable(source_pixels, source_stride, source_start, &weights,
            &mut intermediate[output_row..output_row + intermediate_stride], output_row, cancellation)?;
    }
    drop(weights);
    if cancellation.is_cancelled(RenderWork::Boundary, 0) { return None; }
    let mut output = view.buffer_with_dimensions(width, height);
    let output_pixels: &mut [u8] = output.as_mut();
    let mut weights = Vec::new();
    for output_x in 0..width {
        let first = horizontal.weights_cancellable(output_x, &mut weights, cancellation)? as usize;
        for first_y in (0..height as usize).step_by(RENDER_PIXEL_BLOCK) {
            // The fallback counts the same visits in its column order.
            if cancellation.is_cancelled(RenderWork::ResizeHorizontal, output_x as usize * height as usize + first_y) { return None; }
            let end_y = (first_y + RENDER_PIXEL_BLOCK).min(height as usize);
            for y in first_y..end_y {
                let input_row = y * intermediate_stride;
                let total = if weights.len() > WEIGHT_BLOCK {
                    accumulate_long_weights(&weights, cancellation, RenderWork::ResizeHorizontal,
                        output_x as usize * height as usize + y, |offset| intermediate[input_row + first + offset])?
                } else {
                    accumulate_weights(0.0, &weights, |offset| intermediate[input_row + first + offset])
                };
                output_pixels[y * output_stride + output_x as usize] = nearest_u8(total);
            }
        }
    }
    if cancellation.is_cancelled(RenderWork::Boundary, 0) { return None; }
    Some(output)
}

fn try_row_buffer(width: usize) -> Option<Vec<f32>> {
    #[cfg(test)]
    if streaming_tests::row_reservation_fails() { return None; }
    let mut row = Vec::new();
    row.try_reserve_exact(width).ok()?;
    row.resize(width, 0.0f32);
    #[cfg(test)]
    streaming_tests::record_intermediate_allocation(&row);
    Some(row)
}

fn full_intermediate(width: usize, height: u32) -> Vec<f32> {
    let length = width.checked_mul(height as usize)
        .expect("Triangle intermediate dimensions exceed the addressable size");
    let intermediate = vec![0.0f32; length];
    #[cfg(test)]
    streaming_tests::record_intermediate_allocation(&intermediate);
    intermediate
}

fn write_vertical_row(source: &[u8], source_stride: usize, source_start: usize,
    weights: &[f32], row: &mut [f32])
{
    // Cast bytes directly to f32 with no normalization or inter-pass rounding.
    // Each row is stored before horizontal accumulation, preserving both axes.
    for (x, pixel) in row.iter_mut().enumerate() {
        *pixel = accumulate_weights(0.0, weights,
            |offset| source[source_start + offset * source_stride + x] as f32);
    }
}

fn write_vertical_row_cancellable(source: &[u8], source_stride: usize, source_start: usize,
    weights: &[f32], row: &mut [f32], completed: usize, cancellation: &RenderCancellation<'_>) -> Option<()>
{
    for first_x in (0..row.len()).step_by(RENDER_PIXEL_BLOCK) {
        if cancellation.is_cancelled(RenderWork::ResizeVertical, completed + first_x) { return None; }
        let end_x = (first_x + RENDER_PIXEL_BLOCK).min(row.len());
        for x in first_x..end_x {
            row[x] = if weights.len() > WEIGHT_BLOCK {
                accumulate_long_weights(weights, cancellation, RenderWork::ResizeVertical, completed + x,
                    |offset| source[source_start + offset * source_stride + x] as f32)?
            } else {
                accumulate_weights(0.0, weights,
                    |offset| source[source_start + offset * source_stride + x] as f32)
            };
        }
    }
    Some(())
}

fn accumulate_long_weights(weights: &[f32], cancellation: &RenderCancellation<'_>, work: RenderWork,
    completed: usize, mut sample: impl FnMut(usize) -> f32) -> Option<f32>
{
    let mut total = 0.0f32;
    for (block, weights) in weights.chunks(WEIGHT_BLOCK).enumerate() {
        if cancellation.is_cancelled(work, completed) { return None; }
        total = accumulate_weights(total, weights, |offset| sample(block * WEIGHT_BLOCK + offset));
    }
    Some(total)
}

#[inline]
fn accumulate_weights(mut total: f32, weights: &[f32], mut sample: impl FnMut(usize) -> f32) -> f32 {
    for (offset, weight) in weights.iter().enumerate() { total += sample(offset) * weight; }
    total
}

/// Exact output byte of the reference clamp, round-away and saturating cast.
/// Negative bit patterns and every NaN return zero; positive infinity saturates.
#[inline]
fn nearest_u8(value: f32) -> u8 {
    let bits = value.to_bits();
    if bits < 0x3f00_0000 || bits > 0x7f80_0000 { return 0; }
    if bits >= 0x437f_0000 { return 255; }
    // Remaining x = M / 2^shift is in [0.5, 255), shift is 16..=24,
    // and M + bias < 2^25. Integer half-away rounding cannot overflow.
    let shift = 150 - (bits >> 23);
    let significand = (bits & 0x007f_ffff) | 0x0080_0000;
    ((significand + (1u32 << (shift - 1))) >> shift) as u8
}

struct HorizontalColumn {
    first: usize,
    offset: usize,
    length: usize,
}

struct HorizontalPlan {
    columns: Vec<HorizontalColumn>,
    coefficients: Vec<f32>,
}

enum PlanBuild {
    Ready(HorizontalPlan),
    Fallback,
    Cancelled,
}

fn plan_bytes(columns: usize, coefficients: usize) -> Option<usize> {
    columns.checked_mul(size_of::<HorizontalColumn>())?
        .checked_add(coefficients.checked_mul(size_of::<f32>())?)
}

fn plan_cancelled(cancellation: Option<&RenderCancellation<'_>>, completed_columns: usize) -> bool {
    cancellation.is_some_and(|value| value.is_cancelled(RenderWork::ResizePlan, completed_columns))
}

impl HorizontalPlan {
    fn build(axis: &Axis, width: u32, budget: usize, cancellation: Option<&RenderCancellation<'_>>) -> PlanBuild {
        if plan_cancelled(cancellation, 0) { return PlanBuild::Cancelled; }
        let column_count = width as usize;
        if plan_bytes(column_count, 0).is_none_or(|bytes| bytes > budget) { return PlanBuild::Fallback; }

        // First pass counts exact coefficient slots without allocating. Each
        // plan pass reports completed columns, restarting at zero for filling.
        let mut coefficient_count = 0usize;
        for x in 0..width {
            if x > 0 && x as usize % WEIGHT_BLOCK == 0 && plan_cancelled(cancellation, x as usize) {
                return PlanBuild::Cancelled;
            }
            let range = axis.range(x);
            let Some(count) = coefficient_count.checked_add((range.end - range.first) as usize) else {
                return PlanBuild::Fallback;
            };
            if plan_bytes(column_count, count).is_none_or(|bytes| bytes > budget) { return PlanBuild::Fallback; }
            coefficient_count = count;
        }
        if plan_cancelled(cancellation, column_count) { return PlanBuild::Cancelled; }

        // Reserve each vector once. No temporary per-column weight vector is
        // retained by the plan, and a failed reservation selects the old path.
        let mut plan = Self { columns: Vec::new(), coefficients: Vec::new() };
        if plan.columns.try_reserve_exact(column_count).is_err() { return PlanBuild::Fallback; }
        // An allocator may grant more capacity than requested. Account for the
        // first actual capacity before requesting the second allocation.
        if plan_bytes(plan.columns.capacity(), coefficient_count).is_none_or(|bytes| bytes > budget) {
            return PlanBuild::Fallback;
        }
        if plan_cancelled(cancellation, column_count) { return PlanBuild::Cancelled; }
        if plan.coefficients.try_reserve_exact(coefficient_count).is_err() { return PlanBuild::Fallback; }
        if plan_bytes(plan.columns.capacity(), plan.coefficients.capacity()).is_none_or(|bytes| bytes > budget) {
            return PlanBuild::Fallback;
        }

        if plan_cancelled(cancellation, 0) { return PlanBuild::Cancelled; }
        for x in 0..width {
            if x > 0 && x as usize % WEIGHT_BLOCK == 0 && plan_cancelled(cancellation, x as usize) {
                return PlanBuild::Cancelled;
            }
            let range = axis.range(x);
            let offset = plan.coefficients.len();
            let Some(end) = offset.checked_add((range.end - range.first) as usize) else {
                return PlanBuild::Fallback;
            };
            // Count and fill use the same range calculation. Check the bound
            // before appending so push can never grow this plan implicitly.
            if end > coefficient_count || end > plan.coefficients.capacity() { return PlanBuild::Fallback; }
            if axis.append_weights(range, &mut plan.coefficients, cancellation).is_none() {
                return PlanBuild::Cancelled;
            }
            plan.columns.push(HorizontalColumn { first: range.first as usize, offset,
                length: plan.coefficients.len() - offset });
        }
        if plan_cancelled(cancellation, column_count) { return PlanBuild::Cancelled; }
        PlanBuild::Ready(plan)
    }

    fn weights(&self, column: &HorizontalColumn) -> &[f32] {
        &self.coefficients[column.offset..column.offset + column.length]
    }
}

#[derive(Clone, Copy)]
struct AxisRange {
    first: u32,
    end: u32,
    center: f32,
}

struct Axis {
    source_length: u32,
    ratio: f32,
    sampling_ratio: f32,
    support: f32,
}

impl Axis {
    fn new(source_length: u32, target_length: u32) -> Self {
        let ratio = source_length as f32 / target_length as f32;
        let sampling_ratio = if ratio < 1.0 { 1.0 } else { ratio };
        Self { source_length, ratio, sampling_ratio, support: 1.0 * sampling_ratio }
    }

    fn range(&self, output_position: u32) -> AxisRange {
        let center = (output_position as f32 + 0.5) * self.ratio;
        let first = (center - self.support).floor() as i64;
        let first = first.clamp(0, i64::from(self.source_length) - 1) as u32;
        let end = (center + self.support).ceil() as i64;
        let end = end.clamp(i64::from(first) + 1, i64::from(self.source_length)) as u32;
        AxisRange { first, end, center: center - 0.5 }
    }

    /// Return the first source sample and retain normalized weights in order.
    fn weights(&self, output_position: u32, weights: &mut Vec<f32>) -> u32 {
        let range = self.range(output_position);
        weights.clear();
        self.append_weights(range, weights, None).expect("The default weights have no cancellation source");
        range.first
    }

    fn weights_cancellable(&self, output_position: u32, weights: &mut Vec<f32>,
        cancellation: &RenderCancellation<'_>) -> Option<u32>
    {
        let range = self.range(output_position);
        weights.clear();
        self.append_weights(range, weights, Some(cancellation))?;
        Some(range.first)
    }

    fn append_weights(&self, range: AxisRange, weights: &mut Vec<f32>,
        cancellation: Option<&RenderCancellation<'_>>) -> Option<()>
    {
        let offset = weights.len();
        let mut sum = 0.0f32;
        for block in (range.first..range.end).step_by(WEIGHT_BLOCK) {
            if cancellation.is_some_and(|value| value.is_cancelled(RenderWork::ResizeWeights, (block - range.first) as usize)) { return None; }
            for input_position in block..block.saturating_add(WEIGHT_BLOCK as u32).min(range.end) {
                let distance = (input_position as f32 - range.center) / self.sampling_ratio;
                let weight = triangle(distance);
                weights.push(weight);
                sum += weight;
            }
        }
        for (block, weights) in weights[offset..].chunks_mut(WEIGHT_BLOCK).enumerate() {
            if cancellation.is_some_and(|value| value.is_cancelled(RenderWork::ResizeWeights, block * WEIGHT_BLOCK)) { return None; }
            for weight in weights.iter_mut() { *weight /= sum; }
        }
        Some(())
    }
}

fn triangle(value: f32) -> f32 {
    if value.abs() < 1.0 { 1.0 - value.abs() } else { 0.0 }
}
