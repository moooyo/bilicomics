//! Versioned software dithering for final physical-size Reader images.

use anyhow::Result;
use image::GrayImage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpatialDither {
    #[serde(rename = "off")]
    Off,
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

pub fn apply(image: &mut GrayImage, algorithm: SpatialDither) -> Result<()> {
    apply_with_cancel(image, algorithm, super::check_decode_cancelled)
}

fn apply_with_cancel(
    image: &mut GrayImage,
    algorithm: SpatialDither,
    mut check: impl FnMut() -> Result<()>,
) -> Result<()> {
    check()?;
    let width = image.width() as usize;
    let length = width * image.height() as usize;
    if length == 0 {
        return Ok(());
    }
    let pixels = &mut image.as_mut()[..length];
    match algorithm {
        SpatialDither::Off => {
            for chunk in pixels.chunks_mut(256) {
                check()?;
                for value in chunk {
                    *value = (((u16::from(*value) + 8) / 17) * 17) as u8;
                }
            }
        }
        SpatialDither::Bayer8 => {
            const TABLES: [[u8; 256]; 64] = tone_tables();
            for (y, row) in pixels.chunks_exact_mut(width).enumerate() {
                for (block, chunk) in row.chunks_mut(256).enumerate() {
                    check()?;
                    for (offset, value) in chunk.iter_mut().enumerate() {
                        let x = block * 256 + offset;
                        *value = TABLES[(y % 8) * 8 + x % 8][usize::from(*value)];
                    }
                }
            }
        }
        SpatialDither::FloydSteinberg => {
            let mut current = vec![0_i32; width + 2];
            let mut next = vec![0_i32; width + 2];
            for (y, row) in pixels.chunks_exact_mut(width).enumerate() {
                let reverse = y % 2 != 0;
                for position in 0..width {
                    if position % 256 == 0 {
                        check()?;
                    }
                    let x = if reverse { width - 1 - position } else { position };
                    let value = i32::from(row[x]) * 256 + current[x + 1];
                    let quantized = (value + 2176).div_euclid(4352).clamp(0, 15) * 17;
                    row[x] = quantized as u8;
                    let error = value - quantized * 256;
                    let right = error * 7 / 16;
                    let lower_left = error * 3 / 16;
                    let lower = error * 5 / 16;
                    let lower_right = error - right - lower_left - lower;
                    if reverse {
                        current[x] += right;
                        next[x + 2] += lower_left;
                        next[x + 1] += lower;
                        next[x] += lower_right;
                    } else {
                        current[x + 2] += right;
                        next[x] += lower_left;
                        next[x + 1] += lower;
                        next[x + 2] += lower_right;
                    }
                }
                std::mem::swap(&mut current, &mut next);
                next.fill(0);
            }
        }
    }
    check()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    const ALGORITHMS: [SpatialDither; 3] = [
        SpatialDither::Off, SpatialDither::Bayer8, SpatialDither::FloydSteinberg,
    ];

    #[test]
    fn all_modes_preserve_native_levels_and_do_not_touch_tail_storage() {
        for algorithm in ALGORITHMS {
            let mut storage = Vec::new();
            for _ in 0..9 { storage.extend((0..=15).map(|level| level * 17)); }
            storage.extend([3, 42, 254]);
            let mut image = GrayImage::from_raw(16, 9, storage.clone()).unwrap();
            apply(&mut image, algorithm).unwrap();
            assert_eq!(image.as_raw(), &storage);
            let mut empty = GrayImage::new(0, 0);
            apply(&mut empty, algorithm).unwrap();
        }
    }

    #[test]
    fn off_has_sixteen_uniform_bands() {
        let mut image = GrayImage::from_fn(256, 9, |x, _| Luma([x as u8]));
        apply(&mut image, SpatialDither::Off).unwrap();
        for (x, _, pixel) in image.enumerate_pixels() {
            assert_eq!(pixel.0[0], (((x + 8) / 17) * 17) as u8);
            assert_eq!(pixel, image.get_pixel(x, 0));
        }
        assert_eq!(image.as_raw().iter().collect::<std::collections::HashSet<_>>().len(), 16);
    }

    #[test]
    fn bayer_phase_uses_xy_even_with_odd_widths_and_chunk_boundaries() {
        let mut image = GrayImage::from_pixel(521, 17, Luma([128]));
        apply(&mut image, SpatialDither::Bayer8).unwrap();
        for (x, y, pixel) in image.enumerate_pixels() {
            assert_eq!(pixel, image.get_pixel(x % 8, y % 8));
            assert!(matches!(pixel.0[0], 119 | 136));
        }
        let upper = (0..8).flat_map(|y| (0..8).map(move |x| (x, y)))
            .filter(|(x, y)| image.get_pixel(*x, *y).0[0] == 136).count();
        assert_eq!(upper, 34);
    }

    #[test]
    fn both_dithers_resolve_all_input_tones_with_native_output_levels() {
        for algorithm in [SpatialDither::Bayer8, SpatialDither::FloydSteinberg] {
            let mut previous_sum = None;
            for tone in 0..=255 {
                let mut image = GrayImage::from_pixel(96, 96, Luma([tone]));
                apply(&mut image, algorithm).unwrap();
                assert!(image.as_raw().iter().all(|value| value % 17 == 0));
                let sum: u32 = (16..80).flat_map(|y| (16..80).map(move |x| (x, y)))
                    .map(|(x, y)| u32::from(image.get_pixel(x, y).0[0])).sum();
                if let Some(previous) = previous_sum { assert!(sum > previous); }
                assert!((f64::from(sum) / 4096.0 - f64::from(tone)).abs() < 0.25);
                previous_sum = Some(sum);
            }
        }
    }

    #[test]
    fn cancellation_is_checked_inside_wide_rows() {
        for algorithm in ALGORITHMS {
            let mut image = GrayImage::from_pixel(4096, 2, Luma([128]));
            let mut checks = 0;
            let result = apply_with_cancel(&mut image, algorithm, || {
                checks += 1;
                anyhow::ensure!(checks < 4, "Cancelled test work");
                Ok(())
            });
            assert!(result.is_err());
            assert_eq!(checks, 4);
            assert_eq!(image.get_pixel(2048, 0).0[0], 128);
        }
    }
}
