use super::Framebuffer;

const SENTINEL: u8 = 0x5a;

#[test]
fn output_polarity_covers_the_complete_gray_gradient_without_changing_sources() {
    let gradient = (0..=u8::MAX).collect::<Vec<_>>();
    for bytes_per_pixel in [1, 4] {
        let stride = gradient.len() * bytes_per_pixel + 7;
        for inverted in [false, true] {
            let mut framebuffer = Framebuffer::memory_for_test(
                256, 2, stride, bytes_per_pixel, SENTINEL,
            ).unwrap();
            framebuffer.set_output_inverted(inverted);
            framebuffer.write_line(1, 0..gradient.len(), &gradient);
            let bytes = framebuffer.mapped_bytes_for_test();
            assert!(bytes[..stride].iter().all(|&value| value == SENTINEL));
            for (x, &logical) in gradient.iter().enumerate() {
                let expected = if inverted { 255 - logical } else { logical };
                let start = stride + x * bytes_per_pixel;
                if bytes_per_pixel == 1 {
                    assert_eq!(bytes[start], expected);
                } else {
                    assert_eq!(&bytes[start..start + 4], &[expected, expected, expected, 255]);
                }
            }
            assert!(bytes[stride + gradient.len() * bytes_per_pixel..]
                .iter().all(|&value| value == SENTINEL));
        }
    }
    assert_eq!(gradient, (0..=u8::MAX).collect::<Vec<_>>());
}

#[test]
fn logical_initial_clear_and_gray_fill_preserve_alpha_and_row_padding() {
    for bytes_per_pixel in [1, 4] {
        let stride = 3 * bytes_per_pixel + 5;
        for inverted in [false, true] {
            let mut framebuffer = Framebuffer::memory_for_test(
                3, 2, stride, bytes_per_pixel, SENTINEL,
            ).unwrap();
            framebuffer.set_output_inverted(inverted);
            for logical in [255, 0, 1, 127, 128, 254] {
                framebuffer.fill(logical);
                let expected = if inverted { 255 - logical } else { logical };
                let bytes = framebuffer.mapped_bytes_for_test();
                for y in 0..2 {
                    let row = &bytes[y * stride..(y + 1) * stride];
                    for pixel in row[..3 * bytes_per_pixel].chunks_exact(bytes_per_pixel) {
                        if bytes_per_pixel == 1 {
                            assert_eq!(pixel, &[expected]);
                        } else {
                            assert_eq!(pixel, &[expected, expected, expected, 255]);
                        }
                    }
                    assert!(row[3 * bytes_per_pixel..].iter().all(|&value| value == SENTINEL));
                }
            }
        }
    }
}

#[test]
fn repeated_segmented_dirty_writes_match_a_complete_scene_write() {
    let width = 11;
    let height = 3;
    for bytes_per_pixel in [1, 4] {
        let stride = width * bytes_per_pixel + 5;
        for inverted in [false, true] {
            let mut whole = Framebuffer::memory_for_test(
                width as u32, height as u32, stride, bytes_per_pixel, SENTINEL,
            ).unwrap();
            let mut dirty = Framebuffer::memory_for_test(
                width as u32, height as u32, stride, bytes_per_pixel, SENTINEL,
            ).unwrap();
            whole.set_output_inverted(inverted);
            dirty.set_output_inverted(inverted);
            for y in 0..height {
                let source = (0..width).map(|x| (y * 37 + x * 23) as u8).collect::<Vec<_>>();
                whole.write_line(y, 0..width, &source);
                for span in [0..2, 2..7, 7..width, 2..7] {
                    dirty.write_line(y, span.clone(), &source[span]);
                }
            }
            assert_eq!(whole.mapped_bytes_for_test(), dirty.mapped_bytes_for_test());
            for row in dirty.mapped_bytes_for_test().chunks_exact(stride) {
                assert!(row[width * bytes_per_pixel..].iter().all(|&value| value == SENTINEL));
            }
        }
    }
}

#[test]
fn changing_output_polarity_alone_never_rewrites_the_shared_memory() {
    let mut framebuffer = Framebuffer::memory_for_test(3, 2, 17, 4, SENTINEL).unwrap();
    for inverted in [true, true, false, false] {
        framebuffer.set_output_inverted(inverted);
        assert!(framebuffer.mapped_bytes_for_test().iter().all(|&value| value == SENTINEL));
    }
}

#[test]
fn native_output_compensation_keeps_software_color_intent_independent() {
    let base = [0, 1, 127, 128, 254, 255];
    for native_inverted in [false, true] {
        for software_inverted in [false, true] {
            // This models upstream pixel intent, not an application setting.
            let logical = base.map(|value| if software_inverted { 255 - value } else { value });
            let mut framebuffer = Framebuffer::memory_for_test(6, 1, 6, 1, SENTINEL).unwrap();
            framebuffer.set_output_inverted(native_inverted);
            framebuffer.write_line(0, 0..6, &logical);
            let modeled_panel = framebuffer.mapped_bytes_for_test().iter()
                .map(|&value| if native_inverted { 255 - value } else { value })
                .collect::<Vec<_>>();
            assert_eq!(modeled_panel, logical);
        }
    }
}
