use std::{cell::Cell, hint::black_box, io::Cursor, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Instant};

use image::{DynamicImage, GenericImageView, GrayImage, ImageFormat, Luma, Rgb, RgbImage};

use super::{ReaderCore, ReaderResources, ReaderSettings, RenderCancellation, Viewport, prepare_gray_source, rgb8_to_gray};
use crate::domain::{ImageIndex, PageDescriptor};
use crate::image_integrity::{ReusableImage, decode_image, decode_image_with_reuse};

fn fixture(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |column, row| Rgb([
        (19 * column + 103 * row + 13 * (row ^ column)) as u8,
        ((31 * column) ^ (7 * row + row / 5)) as u8,
        (41 * column + 101 * row + ((column * row) >> 7)) as u8,
    ]))
}

fn generic_reference(image: &DynamicImage, is_cancelled: &dyn Fn() -> bool) -> Option<GrayImage> {
    let mut grayscale = GrayImage::new(image.width(), image.height());
    for (column, row, pixel) in image.pixels() {
        if column == 0 && row % 32 == 0 && is_cancelled() { return None; }
        let alpha = u32::from(pixel[3]);
        let luma = (54 * u32::from(pixel[0]) + 183 * u32::from(pixel[1]) + 19 * u32::from(pixel[2]) + 128) / 256;
        grayscale.put_pixel(column, row, Luma([((luma * alpha + 255 * (255 - alpha) + 127) / 255) as u8]));
    }
    Some(grayscale)
}

#[test]
fn rgb8_rows_preserve_rounding_and_image_bounds() {
    let source = RgbImage::from_raw(5, 1, vec![0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255, 123]).unwrap();
    assert_eq!(rgb8_to_gray(&source, &|| false).unwrap().as_raw(), &[0, 255, 54, 182, 19]);
    for (width, height) in [(1, 1), (1, 65), (17, 33), (257, 31), (1023, 2)] {
        let source = DynamicImage::ImageRgb8(fixture(width, height));
        let gray = rgb8_to_gray(source.as_rgb8().unwrap(), &|| false).unwrap();
        assert_eq!(gray.dimensions(), (width, height));
        assert_eq!(gray.as_raw(), generic_reference(&source, &|| false).unwrap().as_raw());
    }
}

#[test]
fn rgb8_preparation_caches_only_a_complete_uncancelled_source() {
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(fixture(17, 65)).write_to(&mut encoded, ImageFormat::Png).unwrap();
    let (proof, decoded) = decode_image(encoded.get_ref(), None).unwrap();
    assert!(decoded.as_rgb8().is_some());
    let resources = ReaderResources::new(8192);
    for stop in 1..=6 {
        let checks = Cell::new(0);
        let result = prepare_gray_source(&proof, ReusableImage::Decoded(decoded.clone()), &resources, &|| {
            checks.set(checks.get() + 1);
            checks.get() == stop
        });
        if stop <= 5 {
            assert!(result.is_none());
            assert_eq!(checks.get(), stop);
            assert_eq!(resources.cache_usage().entries, 0);
        } else {
            let result = result.unwrap();
            assert_eq!(checks.get(), 5);
            assert_eq!(result.proof(), &proof);
            assert_eq!(result.pixels.as_raw(), generic_reference(&decoded, &|| false).unwrap().as_raw());
            assert!(Arc::ptr_eq(&result.pixels, &resources.cached_source(&proof).unwrap()));
        }
    }
}

#[test]
#[ignore = "Run the RGB8 conversion comparison on test-env in release mode"]
fn measure_rgb8_gray_rows() {
    assert!(!cfg!(debug_assertions), "Use --release for RGB8 measurements");
    let input = DynamicImage::ImageRgb8(fixture(1920, 2560));
    let cancelled = AtomicBool::new(false);
    let convert = |generic: bool| {
        let started = Instant::now();
        let image = black_box(&input);
        let flag = black_box(&cancelled);
        let check = || flag.load(Ordering::Acquire);
        assert!(!check());
        let gray = if generic { generic_reference(image, &check) }
            else { rgb8_to_gray(image.as_rgb8().unwrap(), &check) };
        assert!(!check());
        let gray = black_box(gray).unwrap();
        (started.elapsed().as_nanos(), gray)
    };
    for case in 0..14 {
        let stock_first = case % 2 == 0;
        let first = convert(stock_first);
        let second = convert(!stock_first);
        let (stock, rows) = if stock_first { (first, second) } else { (second, first) };
        assert!(stock.1.as_raw() == rows.1.as_raw(), "RGB8 output differs from the generic reference");
        if case >= 2 {
            println!("rgb8_gray pair={} order={} generic_ns={} rows_ns={}", case - 1,
                if stock_first { "AB" } else { "BA" }, stock.0, rows.0);
        }
    }
}

#[test]
#[ignore = "Run the cold-pixel CPU reader comparison on test-env in release mode"]
fn measure_cold_rgb_jpeg_reader() {
    assert!(!cfg!(debug_assertions), "Use --release for RGB8 measurements");
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(fixture(1920, 2560)).write_to(&mut encoded, ImageFormat::Jpeg).unwrap();
    let encoded = encoded.into_inner();
    let template = ReaderCore::new(ImageIndex {
        episode_id: "rgb-fixture".into(), revision: "rgb-fixture".into(),
        images: vec![PageDescriptor { id: "color-page".into(), index: 0, width: 1920, height: 2560 }],
        ..Default::default()
    }, ReaderSettings::default()).unwrap();
    let flag = AtomicBool::new(false);
    let check = || flag.load(Ordering::Acquire);
    let render = |generic: bool| {
        let resources = ReaderResources::new(0);
        let mut reader = template.clone();
        let started = Instant::now();
        let (proof, source) = decode_image_with_reuse(black_box(&encoded), None,
            |digest| resources.lookup_digest(digest)).unwrap();
        let ReusableImage::Decoded(decoded) = source else { panic!("Expected a software JPEG decode") };
        assert!(!check());
        let gray = if generic { generic_reference(&decoded, &check) }
            else { rgb8_to_gray(decoded.as_rgb8().unwrap(), &check) }.unwrap();
        drop(decoded);
        assert!(!check());
        let gray = Arc::new(gray);
        resources.insert_source(&proof, gray.clone());
        let frame = reader.render_source_identity_cancellable_with_resources(&gray, Viewport::new(1860, 2480),
            Some(proof.sha256()), Some(&proof), &resources, &RenderCancellation::for_flag(&flag), None).unwrap().unwrap();
        let elapsed = started.elapsed().as_nanos();
        assert_eq!((frame.width, frame.height, frame.rgba.len()), (1860, 2480, 1860 * 2480 * 4));
        assert_eq!((resources.cache_usage().entries, resources.processed_cache_usage().entries), (0, 0));
        (elapsed, frame.rgba, reader.anchor(0))
    };
    println!("rgb8_reader jpeg_bytes={} profile=release validation_memo=warm_after_warmup source_and_resize_cache_bytes=0", encoded.len());
    for case in 0..7 {
        let stock_first = case % 2 != 0;
        let first = render(stock_first);
        let second = render(!stock_first);
        let (stock, rows) = if stock_first { (first, second) } else { (second, first) };
        assert!(stock.1 == rows.1, "Complete RGBA frames differ");
        assert_eq!(stock.2, rows.2);
        if case > 0 {
            println!("rgb8_reader pair={case} order={} generic_ns={} rows_ns={}",
                if stock_first { "AB" } else { "BA" }, stock.0, rows.0);
        }
    }
}
