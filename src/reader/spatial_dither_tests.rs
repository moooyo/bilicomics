// SPDX-License-Identifier: GPL-3.0-or-later
// Compatibility properties adapted from the frozen AidokuForKindle handoff.
// Original source and complete license: docs/research/aidoku-spatial-dither-20261116/.

use super::*;
use image::Luma;
use sha2::{Digest, Sha256};
use std::{collections::HashSet, path::Path};

const METHODS: [SpatialDither; 3] = [SpatialDither::Off, SpatialDither::Bayer8, SpatialDither::FloydSteinberg];

fn fixture(name: &str, expected_sha256: &str) -> GrayImage {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs/research/aidoku-spatial-dither-20261116").join(name);
    let encoded = std::fs::read(path).expect("Read the unchanged spatial-dither reference");
    assert_eq!(format!("{:x}", Sha256::digest(&encoded)), expected_sha256,
               "The frozen reference bytes changed: {name}");
    image::load_from_memory_with_format(&encoded, image::ImageFormat::Png).unwrap().into_luma8()
}

#[test]
fn every_method_matches_the_frozen_full_resolution_oracle_pixel_for_pixel() {
    let input = fixture("input-gray8.png", "21ded69522eb01df3bdbd69839a1ca35c95d1d6025ed51429869ed0e6ea416ff");
    assert_eq!(input.dimensions(), (1860, 2480));
    for (method, name, sha256) in [
        (SpatialDither::Off, "expected-off.png", "54f99ac2d2be1b8fd19fdf1b2b4d812053c80ccc036c2d0032bd798788bf8305"),
        (SpatialDither::Bayer8, "expected-bayer8.png", "cd2fb81827fb8e25a322128e63e069a06c64289b079354075c185d245b6e322c"),
        (SpatialDither::FloydSteinberg, "expected-floyd-steinberg-q8.png", "aa2b01a3553ca595d05d2ba4bad43cbfcd456de81198edd9e992b6663db3ac84"),
    ] {
        let expected = fixture(name, sha256);
        let mut actual = input.clone();
        apply(&mut actual, method).unwrap();
        assert_eq!(actual.dimensions(), expected.dimensions());
        let difference = actual.as_raw().iter().zip(expected.as_raw()).position(|(a, b)| a != b);
        assert_eq!(difference, None, "The {method:?} output differs at pixel {difference:?}");
    }
}

#[test]
fn storage_and_cache_identity_use_explicit_versions_and_default_to_bayer8() {
    assert_eq!(SpatialDither::default(), SpatialDither::Bayer8);
    let mut keys = HashSet::new();
    for (method, key, version) in [
        (SpatialDither::Off, "off", "off"),
        (SpatialDither::Bayer8, "bayer8", "bayer8_v1"),
        (SpatialDither::FloydSteinberg, "floyd_steinberg", "floyd_steinberg_q8_v1"),
    ] {
        assert_eq!(method.key(), key);
        assert_eq!(method.cache_key(), version);
        assert_eq!(serde_json::to_string(&method).unwrap(), format!("\"{version}\""));
        assert_eq!(serde_json::from_str::<SpatialDither>(&format!("\"{version}\"")).unwrap(), method);
        assert!(keys.insert(method.cache_key()));
    }
    assert!(serde_json::from_str::<SpatialDither>("\"bayer8\"").is_err());
    assert!(serde_json::from_str::<SpatialDither>("\"floyd_steinberg\"").is_err());
}

#[test]
fn native_levels_tail_storage_metadata_and_empty_images_remain_intact() {
    for method in METHODS {
        let mut storage: Vec<u8> = (0..257 * 9).map(|index| (index % 16 * 17) as u8).collect();
        storage.extend_from_slice(&[3, 42, 254]);
        let mut image = GrayImage::from_raw(257, 9, storage.clone()).unwrap();
        image.set_color_space(image::metadata::Cicp::DISPLAY_P3).unwrap();
        apply(&mut image, method).unwrap();
        assert_eq!(image.as_raw(), &storage);
        assert_eq!(image.color_space(), image::metadata::Cicp::DISPLAY_P3);
        for (width, height) in [(0, 0), (0, 7), (9, 0)] {
            apply(&mut GrayImage::new(width, height), method).unwrap();
        }
    }
}

#[test]
fn off_has_exactly_sixteen_uniform_nearest_level_bands() {
    let mut image = GrayImage::from_fn(256, 9, |x, _| Luma([x as u8]));
    apply(&mut image, SpatialDither::Off).unwrap();
    for (x, _, value) in image.enumerate_pixels() {
        assert_eq!(value[0], (((x + 8) / 17) * 17) as u8);
        assert_eq!(value, image.get_pixel(x, 0));
    }
    assert_eq!(image.as_raw().iter().collect::<HashSet<_>>().len(), 16);
}

#[test]
fn bayer_keeps_the_frozen_xy_orientation_and_odd_width_phase() {
    let mut image = GrayImage::from_pixel(521, 17, Luma([128]));
    apply(&mut image, SpatialDither::Bayer8).unwrap();
    assert_eq!(image.get_pixel(1, 0)[0], 136);
    assert_eq!(image.get_pixel(0, 1)[0], 119);
    for (x, y, value) in image.enumerate_pixels() {
        assert_eq!(value, image.get_pixel(x % 8, y % 8));
        assert!(matches!(value[0], 119 | 136));
    }
    let upper = (0..8).flat_map(|y| (0..8).map(move |x| (x, y)))
        .filter(|&(x, y)| image.get_pixel(x, y)[0] == 136).count();
    assert_eq!(upper, 34);
}

#[test]
fn q8_stencil_preserves_signed_remainders() {
    for error in -4352..=4352 {
        let split = split_error(error);
        assert_eq!(split.into_iter().sum::<i32>(), error);
    }
    assert_eq!(split_error(-1), [0, 0, 0, -1]);
    assert_eq!(split_error(1), [0, 0, 0, 1]);
}

#[test]
fn floyd_handles_single_columns_small_boundaries_and_serpentine_odd_rows() {
    for (width, height, input, expected) in [
        (1, 7, vec![1, 1, 16, 1, 1, 15, 9], vec![0, 0, 17, 0, 0, 17, 17]),
        (2, 2, vec![8, 8, 8, 10], vec![0, 17, 0, 17]),
    ] {
        let mut image = GrayImage::from_raw(width, height, input).unwrap();
        apply(&mut image, SpatialDither::FloydSteinberg).unwrap();
        assert_eq!(image.into_raw(), expected);
    }
    let width = 521;
    let tone = |x: u32| ((x * 47 + x / 7 * 13 + 9) % 256) as u8;
    let mut two_rows = GrayImage::from_fn(width, 2, |x, y| Luma([if y == 0 { 119 } else { tone(x) }]));
    let mut mirrored = GrayImage::from_fn(width, 1, |x, _| Luma([tone(width - 1 - x)]));
    apply(&mut two_rows, SpatialDither::FloydSteinberg).unwrap();
    apply(&mut mirrored, SpatialDither::FloydSteinberg).unwrap();
    for x in 0..width { assert_eq!(two_rows.get_pixel(x, 1), mirrored.get_pixel(width - 1 - x, 0)); }
}

#[test]
fn spatial_methods_resolve_all_256_tones_using_only_native_codes() {
    for method in [SpatialDither::Bayer8, SpatialDither::FloydSteinberg] {
        let mut previous_sum = None;
        for tone in 0..=255 {
            let mut image = GrayImage::from_pixel(96, 96, Luma([tone]));
            apply(&mut image, method).unwrap();
            assert!(image.as_raw().iter().all(|value| value % 17 == 0));
            let sum: u32 = (16..80).flat_map(|y| (16..80).map(move |x| (x, y)))
                .map(|(x, y)| u32::from(image.get_pixel(x, y)[0])).sum();
            if let Some(previous) = previous_sum { assert!(sum > previous); }
            assert!((f64::from(sum) / 4096.0 - f64::from(tone)).abs() < 0.25);
            previous_sum = Some(sum);
        }
    }
}

#[test]
fn cancellation_rejects_work_at_entry_inside_wide_rows_and_after_processing() {
    for method in METHODS {
        let mut image = GrayImage::from_pixel(4096, 2, Luma([128]));
        let original = image.clone();
        let result = apply_with_cancel(&mut image, method, || anyhow::bail!("Cancelled at entry"));
        assert!(result.unwrap_err().to_string().contains("Cancelled at entry"));
        assert_eq!(image.as_raw(), original.as_raw());
        let mut checks = 0;
        let result = apply_with_cancel(&mut image, method, || {
            checks += 1;
            anyhow::ensure!(checks < 4, "Cancelled inside the row");
            Ok(())
        });
        assert!(result.unwrap_err().to_string().contains("Cancelled inside the row"));
        assert_eq!(checks, 4);
        assert_ne!(image.get_pixel(0, 0)[0], 128);
        assert_eq!(image.get_pixel(2048, 0)[0], 128);
    }
    let mut image = GrayImage::from_pixel(1, 1, Luma([128]));
    let mut checks = 0;
    let result = apply_with_cancel(&mut image, SpatialDither::Off, || {
        checks += 1;
        anyhow::ensure!(checks < 3, "Cancelled after processing");
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(image.get_pixel(0, 0)[0], 136);
}
