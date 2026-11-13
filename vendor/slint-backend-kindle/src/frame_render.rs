//! Complete scene rendering for explicit framebuffer replacements.

use slint::Rgb8Pixel;
use slint::platform::software_renderer::{
    PhysicalRegion, RepaintBufferType, SoftwareRenderer,
};

/// A replacement keeps `NewBuffer` active through the actual render, so a
/// cached scene or an unchanged property tree still rebuilds every scene pixel.
/// Ordinary frames retain the renderer's existing dirty-region behavior.
pub(crate) fn render_scene(
    renderer: &SoftwareRenderer,
    buffer: &mut [Rgb8Pixel],
    pixel_stride: usize,
    full_repaint: bool,
) -> PhysicalRegion {
    if full_repaint {
        renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
    }
    let dirty = renderer.render(buffer, pixel_stride);
    if full_repaint {
        renderer.set_repaint_buffer_type(RepaintBufferType::ReusedBuffer);
    }
    dirty
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::software_renderer::MinimalSoftwareWindow;
    use slint::{Color, ComponentHandle, PhysicalPosition, PhysicalSize};
    use std::rc::Rc;

    const WIDTH: usize = 24;
    const HEIGHT: usize = 18;
    const STRIDE: usize = WIDTH + 3;
    const PAPER: u8 = 0xf1;
    const INITIAL_PATCH: u8 = 0x11;
    const SENTINEL: Rgb8Pixel = Rgb8Pixel { r: 0x12, g: 0x53, b: 0xa5 };

    slint::slint! {
        export component CompleteRepaintTestWindow inherits Window {
            width: 24px;
            height: 18px;
            background: #f1f1f1;
            in-out property <color> patch-color: #111111;
            Rectangle {
                x: 3px;
                y: 4px;
                width: 5px;
                height: 4px;
                background: root.patch-color;
            }
        }
    }

    struct HeadlessRenderPlatform(Rc<MinimalSoftwareWindow>);

    impl slint::platform::Platform for HeadlessRenderPlatform {
        fn create_window_adapter(
            &self,
        ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }

    fn scene() -> (Rc<MinimalSoftwareWindow>, CompleteRepaintTestWindow) {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        slint::platform::set_platform(Box::new(HeadlessRenderPlatform(window.clone()))).unwrap();
        let component = CompleteRepaintTestWindow::new().unwrap();
        component.show().unwrap();
        window.set_size(PhysicalSize::new(WIDTH as u32, HEIGHT as u32));
        (window, component)
    }

    fn draw(
        window: &MinimalSoftwareWindow,
        pixels: &mut [Rgb8Pixel],
        full_repaint: bool,
    ) -> PhysicalRegion {
        window.request_redraw();
        let mut region = None;
        window.draw_if_needed(|renderer| {
            region = Some(render_scene(renderer, pixels, STRIDE, full_repaint));
            assert_eq!(renderer.repaint_buffer_type(), RepaintBufferType::ReusedBuffer);
        });
        region.expect("a requested frame must execute the real software renderer")
    }

    fn set_patch(component: &CompleteRepaintTestWindow, value: u8) {
        component.set_patch_color(Color::from_rgb_u8(value, value, value));
    }

    fn is_patch(x: usize, y: usize) -> bool {
        (3..8).contains(&x) && (4..8).contains(&y)
    }

    fn assert_scene(pixels: &[Rgb8Pixel], patch: u8) {
        for y in 0..HEIGHT {
            for x in 0..STRIDE {
                let expected = if x >= WIDTH {
                    SENTINEL
                } else {
                    let value = if is_patch(x, y) { patch } else { PAPER };
                    Rgb8Pixel { r: value, g: value, b: value }
                };
                let actual = pixels[y * STRIDE + x];
                assert_eq!((actual.r, actual.g, actual.b),
                    (expected.r, expected.g, expected.b), "pixel ({x}, {y})");
            }
        }
    }

    fn assert_complete(region: &PhysicalRegion) {
        assert_eq!(region.bounding_box_origin(), PhysicalPosition::new(0, 0));
        assert_eq!(region.bounding_box_size(), PhysicalSize::new(WIDTH as u32, HEIGHT as u32));
    }

    #[test]
    fn complete_repaint_rebuilds_every_pixel_after_a_small_scene_change() {
        let (window, component) = scene();
        let mut pixels = vec![SENTINEL; STRIDE * HEIGHT];
        assert_complete(&draw(&window, &mut pixels, false));
        assert_scene(&pixels, INITIAL_PATCH);

        set_patch(&component, 0x55);
        let partial = draw(&window, &mut pixels, false);
        let size = partial.bounding_box_size();
        assert!(size.width > 0 && size.width < WIDTH as u32);
        assert!(size.height > 0 && size.height < HEIGHT as u32);
        assert_scene(&pixels, 0x55);

        set_patch(&component, 0xbb);
        assert_complete(&draw(&window, &mut pixels, true));
        assert_scene(&pixels, 0xbb);
        component.hide().unwrap();
    }

    #[test]
    fn complete_repaint_rebuilds_an_unchanged_scene_over_a_replaced_buffer() {
        let (window, component) = scene();
        let mut pixels = vec![SENTINEL; STRIDE * HEIGHT];
        draw(&window, &mut pixels, false);
        assert_scene(&pixels, INITIAL_PATCH);

        // No UI property changed. A complete replacement must not reuse the
        // old dirty-region decision when the destination pixels are new.
        pixels.fill(SENTINEL);
        assert_complete(&draw(&window, &mut pixels, true));
        assert_scene(&pixels, INITIAL_PATCH);
        component.hide().unwrap();
    }

    #[test]
    fn ordinary_repaint_preserves_reused_buffer_dirty_tracking() {
        let (window, component) = scene();
        let mut pixels = vec![SENTINEL; STRIDE * HEIGHT];
        draw(&window, &mut pixels, false);
        pixels.fill(SENTINEL);

        set_patch(&component, 0x77);
        let region = draw(&window, &mut pixels, false);
        let origin = region.bounding_box_origin();
        let size = region.bounding_box_size();
        assert!(size.width > 0 && size.width < WIDTH as u32);
        assert!(size.height > 0 && size.height < HEIGHT as u32);
        assert!(origin.x > 0 && origin.y > 0);
        assert_eq!((pixels[0].r, pixels[0].g, pixels[0].b),
            (SENTINEL.r, SENTINEL.g, SENTINEL.b));
        let patch = pixels[5 * STRIDE + 4];
        assert_eq!((patch.r, patch.g, patch.b), (0x77, 0x77, 0x77));
        component.hide().unwrap();
    }
}
