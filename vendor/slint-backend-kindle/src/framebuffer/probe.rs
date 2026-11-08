//! Read-only framebuffer metadata used to observe native output presentation.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

use super::FramebufferLayout;
use super::ffi::{FBIOGET_FSCREENINFO, FBIOGET_VSCREENINFO, FbFixScreeninfo, FbVarScreeninfo};
use crate::NativeFramebufferObservation;

const GRAYSCALE_8BIT: u32 = 1;
const GRAYSCALE_8BIT_INVERTED: u32 = 2;

/// Observe only known 8-bit native formats without acquiring display ownership.
///
/// The descriptor is read-only. This does not map memory, write pixels, submit
/// updates, or open input. Unknown formats return `None` for a caller-owned
/// legacy fallback; invalid geometry and failed I/O remain errors.
/// A retained mapping must match fresh geometry, stride, and pixel depth before
/// that fallback is considered; this never remaps or modifies retained memory.
pub(crate) fn probe_output_inversion(
    expected_geometry: Option<(u32, u32)>,
    retained_layout: Option<FramebufferLayout>,
) -> io::Result<Option<bool>> {
    probe_with_query(expected_geometry, retained_layout, read_information)
}

/// Observe raw rotation and polarity through the same fresh metadata snapshot.
///
/// This uses no pixel mapping, input device, SET ioctl, or display update. A
/// retained mapping is validated before admitting either part of the result.
pub(crate) fn probe_native_framebuffer(
    expected_geometry: Option<(u32, u32)>,
    retained_layout: Option<FramebufferLayout>,
) -> io::Result<NativeFramebufferObservation> {
    probe_native_with_query(expected_geometry, retained_layout, read_information)
}

fn read_information() -> io::Result<(FbVarScreeninfo, FbFixScreeninfo)> {
    let file = File::open("/dev/fb0")?;
    let mut variable = FbVarScreeninfo::default();
    // SAFETY: GET copies the existing ABI structure into this aligned writable
    // buffer and does not retain its pointer or change framebuffer state.
    if unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            FBIOGET_VSCREENINFO as _,
            &mut variable as *mut FbVarScreeninfo,
        )
    } == -1
    {
        return Err(io::Error::last_os_error());
    }
    let mut fixed = FbFixScreeninfo::default();
    // SAFETY: This GET uses the matching fixed-info ABI and the same read-only
    // descriptor. The output structure remains alive for the entire call.
    if unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            FBIOGET_FSCREENINFO as _,
            &mut fixed as *mut FbFixScreeninfo,
        )
    } == -1
    {
        return Err(io::Error::last_os_error());
    }
    Ok((variable, fixed))
}

fn probe_with_query(
    expected_geometry: Option<(u32, u32)>,
    retained_layout: Option<FramebufferLayout>,
    query: impl FnOnce() -> io::Result<(FbVarScreeninfo, FbFixScreeninfo)>,
) -> io::Result<Option<bool>> {
    let (variable, fixed) = query()?;
    validate_retained_layout(&variable, &fixed, retained_layout)?;
    decode_output_inversion(&variable, &fixed, expected_geometry)
}

fn probe_native_with_query(
    expected_geometry: Option<(u32, u32)>,
    retained_layout: Option<FramebufferLayout>,
    query: impl FnOnce() -> io::Result<(FbVarScreeninfo, FbFixScreeninfo)>,
) -> io::Result<NativeFramebufferObservation> {
    let (variable, fixed) = query()?;
    validate_retained_layout(&variable, &fixed, retained_layout)?;
    let output_inverted = decode_output_inversion(&variable, &fixed, expected_geometry)?;
    if variable.rotate > 3 {
        return Err(invalid(
            "framebuffer rotation must be a kernel quarter-turn value",
        ));
    }
    Ok(NativeFramebufferObservation {
        output_inverted,
        rotation: variable.rotate,
    })
}

fn validate_retained_layout(
    variable: &FbVarScreeninfo,
    fixed: &FbFixScreeninfo,
    retained_layout: Option<FramebufferLayout>,
) -> io::Result<()> {
    if let Some(retained) = retained_layout {
        if (variable.xres, variable.yres) != (retained.width, retained.height) {
            return Err(invalid(
                "framebuffer visible geometry changed from the retained mapping",
            ));
        }
        let current_stride = usize::try_from(fixed.line_length)
            .map_err(|_| invalid("framebuffer stride exceeds the addressable range"))?;
        if current_stride != retained.stride {
            return Err(invalid(
                "framebuffer stride changed from the retained mapping",
            ));
        }
        let current_bytes_per_pixel = usize::try_from(variable.bits_per_pixel.div_ceil(8))
            .map_err(|_| invalid("framebuffer pixel size exceeds the addressable range"))?;
        if variable.bits_per_pixel != retained.bits_per_pixel
            || current_bytes_per_pixel != retained.bytes_per_pixel
        {
            return Err(invalid(
                "framebuffer pixel depth changed from the retained mapping",
            ));
        }
    }
    Ok(())
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn validate_layout(
    width: usize,
    height: usize,
    stride: usize,
    bits_per_pixel: usize,
) -> io::Result<()> {
    if width == 0 || height == 0 || stride == 0 {
        return Err(invalid("framebuffer dimensions and stride must be nonzero"));
    }
    let row_bits = width
        .checked_mul(bits_per_pixel)
        .ok_or_else(|| invalid("framebuffer row size overflows"))?;
    let row_bytes = row_bits
        .checked_add(7)
        .ok_or_else(|| invalid("framebuffer packed row size overflows"))?
        / 8;
    if stride < row_bytes {
        return Err(invalid(
            "framebuffer stride is smaller than the visible row",
        ));
    }
    let visible_bytes = stride
        .checked_mul(height)
        .ok_or_else(|| invalid("framebuffer visible size overflows"))?;
    if visible_bytes > isize::MAX as usize {
        return Err(invalid(
            "framebuffer visible size exceeds the addressable slice bound",
        ));
    }
    Ok(())
}

fn decode_output_inversion(
    variable: &FbVarScreeninfo,
    fixed: &FbFixScreeninfo,
    expected_geometry: Option<(u32, u32)>,
) -> io::Result<Option<bool>> {
    if expected_geometry.is_some_and(|expected| expected != (variable.xres, variable.yres)) {
        return Err(invalid(
            "framebuffer visible geometry does not match the expected geometry",
        ));
    }
    if variable.xoffset != 0 || variable.yoffset != 0 {
        return Err(invalid("framebuffer panning offsets must be zero"));
    }
    let width = usize::try_from(variable.xres)
        .map_err(|_| invalid("framebuffer width exceeds the addressable range"))?;
    let height = usize::try_from(variable.yres)
        .map_err(|_| invalid("framebuffer height exceeds the addressable range"))?;
    let stride = usize::try_from(fixed.line_length)
        .map_err(|_| invalid("framebuffer stride exceeds the addressable range"))?;
    let bits_per_pixel = usize::try_from(variable.bits_per_pixel)
        .map_err(|_| invalid("framebuffer depth exceeds the addressable range"))?;
    validate_layout(width, height, stride, bits_per_pixel)?;
    Ok(match (variable.bits_per_pixel, variable.grayscale) {
        (8, GRAYSCALE_8BIT) => Some(false),
        (8, GRAYSCALE_8BIT_INVERTED) => Some(true),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn information(
        width: u32,
        height: u32,
        depth: u32,
        grayscale: u32,
        stride: u32,
    ) -> (FbVarScreeninfo, FbFixScreeninfo) {
        (
            FbVarScreeninfo {
                xres: width,
                yres: height,
                xres_virtual: width,
                yres_virtual: height,
                bits_per_pixel: depth,
                grayscale,
                ..Default::default()
            },
            FbFixScreeninfo {
                visual: 1,
                line_length: stride,
                ..Default::default()
            },
        )
    }

    #[test]
    fn known_eight_bit_formats_use_the_native_grayscale_code() {
        for (grayscale, expected) in [(GRAYSCALE_8BIT, false), (GRAYSCALE_8BIT_INVERTED, true)] {
            let (variable, mut fixed) = information(17, 11, 8, grayscale, 20);
            for visual in [0, 1, 2, 3] {
                fixed.visual = visual;
                assert_eq!(
                    decode_output_inversion(&variable, &fixed, Some((17, 11))).unwrap(),
                    Some(expected)
                );
            }
        }
    }

    #[test]
    fn unknown_formats_preserve_the_legacy_fallback() {
        for (depth, grayscale) in [
            (8, 0),
            (8, 3),
            (8, u32::MAX),
            (32, 1),
            (32, 2),
            (16, 2),
            (4, 2),
            (0, 2),
        ] {
            let (variable, fixed) = information(17, 11, depth, grayscale, 68);
            assert_eq!(
                decode_output_inversion(&variable, &fixed, None).unwrap(),
                None
            );
        }
    }

    #[test]
    fn expected_geometry_is_exact_without_a_device_profile_assumption() {
        for geometry in [(3, 2), (17, 11), (1272, 1696)] {
            let (variable, fixed) = information(geometry.0, geometry.1, 8, 2, geometry.0);
            assert_eq!(
                decode_output_inversion(&variable, &fixed, Some(geometry)).unwrap(),
                Some(true)
            );
            for expected in [(geometry.0 + 1, geometry.1), (geometry.0, geometry.1 + 1)] {
                let error = decode_output_inversion(&variable, &fixed, Some(expected)).unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            }
        }
    }

    #[test]
    fn invalid_dimensions_and_panning_remain_errors_even_for_unknown_formats() {
        for (width, height) in [(0, 11), (17, 0)] {
            let (variable, fixed) = information(width, height, 8, 2, 20);
            assert_eq!(
                decode_output_inversion(&variable, &fixed, None)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
        for (xoffset, yoffset) in [(1, 0), (0, 1), (1, 1)] {
            let (mut variable, fixed) = information(17, 11, 32, 2, 68);
            variable.xoffset = xoffset;
            variable.yoffset = yoffset;
            assert_eq!(
                decode_output_inversion(&variable, &fixed, None)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
    }

    #[test]
    fn stride_checks_cover_grayscale_packed_pixels_and_color_fallback() {
        for (depth, stride) in [(8, 16), (32, 67), (8, 0)] {
            let (variable, fixed) = information(17, 11, depth, 2, stride);
            assert_eq!(
                decode_output_inversion(&variable, &fixed, None)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
        let (variable, fixed) = information(3, 2, 4, 2, 2);
        assert_eq!(
            decode_output_inversion(&variable, &fixed, None).unwrap(),
            None
        );
        let (_, short) = information(3, 2, 4, 2, 1);
        assert_eq!(
            decode_output_inversion(&variable, &short, None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn padding_and_a_larger_virtual_surface_do_not_change_visible_geometry() {
        let (mut variable, fixed) = information(17, 11, 8, 2, 24);
        variable.yres_virtual = 99;
        assert_eq!(
            decode_output_inversion(&variable, &fixed, Some((17, 11))).unwrap(),
            Some(true)
        );
    }

    #[test]
    fn row_and_visible_extent_arithmetic_rejects_overflow() {
        for (width, height, stride, depth, reason) in [
            (usize::MAX, 1, usize::MAX, 8, "row size overflows"),
            (usize::MAX, 1, usize::MAX, 1, "packed row size overflows"),
            (1, 2, usize::MAX, 8, "visible size overflows"),
            (1, 1, usize::MAX, 8, "addressable slice bound"),
        ] {
            let error = validate_layout(width, height, stride, depth).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(error.to_string().contains(reason));
        }
    }

    #[test]
    fn injected_query_failures_keep_their_io_error_identity() {
        for errno in [libc::ENOENT, libc::EACCES, libc::ENOTTY, libc::EIO] {
            let error = probe_with_query(None, None, || Err(io::Error::from_raw_os_error(errno)))
                .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(errno));
        }
        let observed =
            probe_with_query(Some((17, 11)), None, || Ok(information(17, 11, 8, 2, 20))).unwrap();
        assert_eq!(observed, Some(true));
    }

    #[test]
    fn a_retained_mapping_rejects_stride_or_depth_changes_even_with_identical_geometry() {
        let retained = FramebufferLayout {
            width: 9,
            height: 11,
            stride: 40,
            bytes_per_pixel: 1,
            bits_per_pixel: 8,
        };
        for (depth, stride, expected_reason) in [
            (8, 39, "stride changed"),
            (16, 40, "pixel depth changed"),
            (32, 40, "pixel depth changed"),
            (4, 40, "pixel depth changed"),
        ] {
            let error = probe_with_query(Some((9, 11)), Some(retained), || {
                Ok(information(9, 11, depth, 2, stride))
            })
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(error.to_string().contains(expected_reason));
        }
        // A polarity change is valid when the retained memory layout is exact.
        let observed = probe_with_query(Some((9, 11)), Some(retained), || {
            Ok(information(9, 11, 8, 1, 40))
        })
        .unwrap();
        assert_eq!(observed, Some(false));
        let inconsistent_pixel_size = FramebufferLayout {
            bytes_per_pixel: 4,
            ..retained
        };
        let error = probe_with_query(Some((9, 11)), Some(inconsistent_pixel_size), || {
            Ok(information(9, 11, 8, 2, 40))
        })
        .unwrap_err();
        assert!(error.to_string().contains("pixel depth changed"));
    }

    #[test]
    fn native_observation_preserves_each_raw_rotation_and_unknown_format() {
        for rotation in 0..=3 {
            for (depth, grayscale, expected) in
                [(8, 1, Some(false)), (8, 2, Some(true)), (32, 1, None)]
            {
                let observed = probe_native_with_query(Some((17, 11)), None, || {
                    let (mut variable, fixed) = information(17, 11, depth, grayscale, 68);
                    variable.rotate = rotation;
                    Ok((variable, fixed))
                })
                .unwrap();
                assert_eq!(
                    observed,
                    NativeFramebufferObservation {
                        output_inverted: expected,
                        rotation
                    }
                );
            }
        }
    }

    #[test]
    fn strict_native_rotation_validation_does_not_change_the_polarity_only_api() {
        for rotation in [4, 90, 180, u32::MAX] {
            for (depth, expected) in [(8, Some(true)), (32, None)] {
                let query = || {
                    let (mut variable, fixed) = information(17, 11, depth, 2, 68);
                    variable.rotate = rotation;
                    Ok((variable, fixed))
                };
                assert_eq!(probe_with_query(None, None, query).unwrap(), expected);
                let error = probe_native_with_query(None, None, query).unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::InvalidData);
                assert!(error.to_string().contains("rotation"));
            }
        }
    }

    #[test]
    fn native_observation_rejects_retained_layout_changes_before_returning_rotation() {
        let retained = FramebufferLayout {
            width: 9,
            height: 11,
            stride: 40,
            bytes_per_pixel: 1,
            bits_per_pixel: 8,
        };
        for (width, height, depth, stride) in [
            (10, 11, 8, 40),
            (9, 12, 8, 40),
            (9, 11, 8, 39),
            (9, 11, 32, 40),
        ] {
            let error = probe_native_with_query(Some((9, 11)), Some(retained), || {
                let (mut variable, fixed) = information(width, height, depth, 2, stride);
                variable.rotate = 1;
                Ok((variable, fixed))
            })
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(error.to_string().contains("retained mapping"));
        }
        for errno in [libc::EACCES, libc::ENOTTY, libc::EIO] {
            let error =
                probe_native_with_query(None, None, || Err(io::Error::from_raw_os_error(errno)))
                    .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(errno));
        }
    }
}
