// SPDX-License-Identifier: GPL-3.0-or-later
// Adapted from the frozen AidokuForKindle spatial-dither handoff of 2026-11-16.
// Original input SHA-256: 182a692d45510bc862bbd6ff3780301600eba892559ac7f3893630e69fd27495.
// Source snapshot: 25df24627f3db1dc3a334b1af6f30246744e3fda9a679191e15711f43c4fde97.
// Source and complete license: docs/research/aidoku-spatial-dither-20261116/.

//! Versioned CPU quantization for an already resized and contrasted Gray8 image.
//!
//! All modes emit the sixteen native codes without enabling Y4 or applying any
//! subsequent quantizer. Cancellation can leave modified pixels; the caller
//! must discard the entire candidate after an error rather than publishing it.

use anyhow::{Context, Result};
use image::GrayImage;
use serde::{Deserialize, Serialize};

const WORK_BLOCK: usize = 256;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpatialDither {
    #[serde(rename = "off")]
    Off,
    #[default]
    #[serde(rename = "bayer8_v1")]
    Bayer8,
    #[serde(rename = "floyd_steinberg_q8_v1")]
    FloydSteinberg,
}

impl SpatialDither {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Bayer8 => "bayer8",
            Self::FloydSteinberg => "floyd_steinberg",
        }
    }

    pub const fn cache_key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Bayer8 => "bayer8_v1",
            Self::FloydSteinberg => "floyd_steinberg_q8_v1",
        }
    }
}

const fn bayer_rank(x: usize, y: usize) -> usize {
    let mut rank = 0;
    let mut bit = 0;
    while bit < 3 {
        rank = rank * 4 + [0, 2, 3, 1][((y >> bit) & 1) * 2 + ((x >> bit) & 1)];
        bit += 1;
    }
    rank
}

const fn tone_tables() -> [[u8; 256]; 64] {
    let mut tables = [[0; 256]; 64];
    let mut phase = 0;
    while phase < 64 {
        let rank = bayer_rank(phase % 8, phase / 8);
        let mut value = 0;
        while value < 256 {
            let upper_count = ((value % 17) * 64 + 8) / 17;
            let level = value / 17 + if rank < upper_count { 1 } else { 0 };
            tables[phase][value] = ((if level > 15 { 15 } else { level }) * 17) as u8;
            value += 1;
        }
        phase += 1;
    }
    tables
}

#[inline]
fn split_error(error: i32) -> [i32; 4] {
    // Keep signed division toward zero and assign the exact remainder to the
    // final diagonal. Replacing these divisions with floor changes the image.
    let forward = error * 7 / 16;
    let lower_back = error * 3 / 16;
    let lower = error * 5 / 16;
    [forward, lower_back, lower, error - forward - lower_back - lower]
}

fn error_row(length: usize) -> Result<Vec<i32>> {
    let mut row = Vec::new();
    row.try_reserve_exact(length).context("Allocate the dithering error row")?;
    row.resize(length, 0);
    Ok(row)
}

/// Apply one selected CPU method, without a caller cancellation source.
pub fn apply(image: &mut GrayImage, method: SpatialDither) -> Result<()> {
    apply_with_cancel(image, method, || Ok(()))
}

/// The caller owns cancellation and must reject every errored candidate.
/// Pixel processing checks cancellation within each 256-pixel work block.
pub(crate) fn apply_with_cancel(
    image: &mut GrayImage,
    method: SpatialDither,
    mut check: impl FnMut() -> Result<()>,
) -> Result<()> {
    check()?;
    let width = usize::try_from(image.width()).context("Dithering width exceeds the address space")?;
    let height = usize::try_from(image.height()).context("Dithering height exceeds the address space")?;
    let length = width.checked_mul(height).context("Dithering image dimensions overflow")?;
    if length == 0 {
        return Ok(());
    }
    let storage: &mut [u8] = image.as_mut();
    let pixels = storage.get_mut(..length).context("Dithering image storage is incomplete")?;
    match method {
        SpatialDither::Off => {
            for chunk in pixels.chunks_mut(WORK_BLOCK) {
                check()?;
                for value in chunk {
                    *value = (((u16::from(*value) + 8) / 17) * 17) as u8;
                }
            }
        }
        SpatialDither::Bayer8 => {
            const TABLES: [[u8; 256]; 64] = tone_tables();
            for (y, row) in pixels.chunks_exact_mut(width).enumerate() {
                for (block, chunk) in row.chunks_mut(WORK_BLOCK).enumerate() {
                    check()?;
                    for (offset, value) in chunk.iter_mut().enumerate() {
                        let x = block * WORK_BLOCK + offset;
                        *value = TABLES[(y % 8) * 8 + x % 8][usize::from(*value)];
                    }
                }
            }
        }
        SpatialDither::FloydSteinberg => {
            let row_length = width.checked_add(2).context("Dithering row guards overflow")?;
            let mut current = error_row(row_length)?;
            let mut next = error_row(row_length)?;
            for (y, row) in pixels.chunks_exact_mut(width).enumerate() {
                let reverse = y % 2 != 0;
                for position in 0..width {
                    if position % WORK_BLOCK == 0 { check()?; }
                    let x = if reverse { width - 1 - position } else { position };
                    let value = i32::from(row[x]) * 256 + current[x + 1];
                    let quantized = (value + 2176).div_euclid(4352).clamp(0, 15) * 17;
                    row[x] = quantized as u8;
                    let [forward, lower_back, lower, lower_forward] = split_error(value - quantized * 256);
                    if reverse {
                        current[x] += forward;
                        next[x + 2] += lower_back;
                        next[x + 1] += lower;
                        next[x] += lower_forward;
                    } else {
                        current[x + 2] += forward;
                        next[x] += lower_back;
                        next[x + 1] += lower;
                        next[x + 2] += lower_forward;
                    }
                }
                std::mem::swap(&mut current, &mut next);
                // Guard cells absorb off-image spill. Clearing the previous
                // row also remains interruptible for unusually wide images.
                for chunk in next.chunks_mut(WORK_BLOCK) {
                    check()?;
                    chunk.fill(0);
                }
            }
        }
    }
    check()
}

#[cfg(test)]
#[path = "spatial_dither_tests.rs"]
mod tests;
