//! Project-owned queued refresh extension; see PATCHES.md for provenance.

use std::fmt;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const MAX_PENDING: usize = 32;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RefreshWaveform {
    #[default]
    Auto,
    Gc16,
    Du,
    A2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshArea {
    /// The dirty pixels from this frame. No dirty pixels means no update.
    Dirty,
    Full,
    /// Window physical coordinates, before the backend's render rotation.
    Rect(PhysicalRect),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefreshRequest {
    pub waveform: RefreshWaveform,
    pub flashing: bool,
    pub area: RefreshArea,
}

impl RefreshRequest {
    pub const fn full() -> Self {
        Self {
            waveform: RefreshWaveform::Gc16,
            flashing: true,
            area: RefreshArea::Full,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshError {
    EmptyRegion,
    EventLoopTerminated,
    QueuePoisoned,
    DisplaySuspended,
}

impl fmt::Display for RefreshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyRegion => "refresh rectangle has no area",
            Self::EventLoopTerminated => "the Kindle event loop has terminated",
            Self::QueuePoisoned => "the refresh queue lock was poisoned",
            Self::DisplaySuspended => "the Kindle display is suspended",
        })
    }
}

impl std::error::Error for RefreshError {}

#[derive(Default)]
pub(crate) struct PendingRefresh {
    requests: Vec<RefreshRequest>,
    overflowed: bool,
}

impl PendingRefresh {
    fn push(&mut self, request: RefreshRequest) {
        if self.overflowed {
            return;
        }
        if self.requests.len() == MAX_PENDING {
            // A stalled producer must not grow an unbounded allocation. A full
            // refresh conservatively includes all requests already accepted.
            self.requests.clear();
            self.requests.push(RefreshRequest::full());
            self.overflowed = true;
        } else {
            self.requests.push(request);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }

    pub(crate) fn take(&mut self) -> Self {
        std::mem::take(self)
    }
}

/// A thread-safe producer. Success acknowledges queueing, not a panel result.
#[derive(Clone)]
pub struct RefreshHandle {
    pub(crate) pending: Arc<Mutex<PendingRefresh>>,
    pub(crate) wake: Arc<OwnedFd>,
    pub(crate) quit: Arc<AtomicBool>,
    pub(crate) suspended: Arc<AtomicBool>,
}

impl RefreshHandle {
    pub fn request(&self, request: RefreshRequest) -> Result<(), RefreshError> {
        if let RefreshArea::Rect(rect) = request.area {
            if rect.width == 0 || rect.height == 0 {
                return Err(RefreshError::EmptyRegion);
            }
        }
        if self.quit.load(Ordering::SeqCst) {
            return Err(RefreshError::EventLoopTerminated);
        }
        if self.suspended.load(Ordering::SeqCst) {
            return Err(RefreshError::DisplaySuspended);
        }
        {
            let mut pending = self
                .pending
                .lock()
                .map_err(|_| RefreshError::QueuePoisoned)?;
            if self.quit.load(Ordering::SeqCst) {
                return Err(RefreshError::EventLoopTerminated);
            }
            if self.suspended.load(Ordering::SeqCst) {
                return Err(RefreshError::DisplaySuspended);
            }
            pending.push(request);
        }
        crate::wakeup::poke(&self.wake);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedRefresh {
    pub region: PhysicalRect,
    pub waveform: RefreshWaveform,
    pub flashing: bool,
}

/// Resolve only after the current pixels have been written to the framebuffer.
pub(crate) fn resolve(
    pending: PendingRefresh,
    dirty: Option<PhysicalRect>,
    panel_width: u32,
    panel_height: u32,
    rotation: u32,
) -> Option<ResolvedRefresh> {
    let dirty = dirty.and_then(|rect| clamp(rect, panel_width, panel_height));
    let mut region = None;
    let mut waveform = RefreshWaveform::A2;
    let mut flashing = false;
    for request in pending.requests {
        let requested_region = match request.area {
            RefreshArea::Dirty => dirty,
            RefreshArea::Full => clamp(
                PhysicalRect {
                    x: 0,
                    y: 0,
                    width: panel_width,
                    height: panel_height,
                },
                panel_width,
                panel_height,
            ),
            RefreshArea::Rect(rect) => rotate_and_clamp(rect, panel_width, panel_height, rotation),
        };
        let Some(requested_region) = requested_region else {
            continue;
        };
        region = Some(region.map_or(requested_region, |previous| {
            union(previous, requested_region)
        }));
        if priority(request.waveform) > priority(waveform) {
            waveform = request.waveform;
        }
        flashing |= request.flashing;
    }
    let Some(mut region) = region else {
        return dirty.map(|region| ResolvedRefresh {
            region,
            waveform: RefreshWaveform::Auto,
            flashing: false,
        });
    };
    if let Some(dirty) = dirty {
        region = union(region, dirty);
    }
    if flashing {
        waveform = RefreshWaveform::Gc16;
    }
    Some(ResolvedRefresh {
        region,
        waveform,
        flashing,
    })
}

fn priority(waveform: RefreshWaveform) -> u8 {
    match waveform {
        RefreshWaveform::A2 => 0,
        RefreshWaveform::Du => 1,
        RefreshWaveform::Auto => 2,
        RefreshWaveform::Gc16 => 3,
    }
}

fn clamp(rect: PhysicalRect, width: u32, height: u32) -> Option<PhysicalRect> {
    if width > i32::MAX as u32 || height > i32::MAX as u32 {
        return None;
    }
    let x0 = i64::from(rect.x).clamp(0, i64::from(width));
    let y0 = i64::from(rect.y).clamp(0, i64::from(height));
    let x1 = (i64::from(rect.x) + i64::from(rect.width)).clamp(0, i64::from(width));
    let y1 = (i64::from(rect.y) + i64::from(rect.height)).clamp(0, i64::from(height));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(PhysicalRect {
        x: x0 as i32,
        y: y0 as i32,
        width: (x1 - x0) as u32,
        height: (y1 - y0) as u32,
    })
}

fn rotate_and_clamp(
    rect: PhysicalRect,
    width: u32,
    height: u32,
    rotation: u32,
) -> Option<PhysicalRect> {
    let transposed = matches!(rotation % 360, 90 | 270);
    let (window_width, window_height) = if transposed {
        (height, width)
    } else {
        (width, height)
    };
    let rect = clamp(rect, window_width, window_height)?;
    let rotated = match rotation % 360 {
        90 => PhysicalRect {
            x: (width - rect.y as u32 - rect.height) as i32,
            y: rect.x,
            width: rect.height,
            height: rect.width,
        },
        180 => PhysicalRect {
            x: (width - rect.x as u32 - rect.width) as i32,
            y: (height - rect.y as u32 - rect.height) as i32,
            ..rect
        },
        270 => PhysicalRect {
            x: rect.y,
            y: (height - rect.x as u32 - rect.width) as i32,
            width: rect.height,
            height: rect.width,
        },
        _ => rect,
    };
    clamp(rotated, width, height)
}

fn union(first: PhysicalRect, second: PhysicalRect) -> PhysicalRect {
    let x = first.x.min(second.x);
    let y = first.y.min(second.y);
    let right = (i64::from(first.x) + i64::from(first.width))
        .max(i64::from(second.x) + i64::from(second.width));
    let bottom = (i64::from(first.y) + i64::from(first.height))
        .max(i64::from(second.y) + i64::from(second.height));
    PhysicalRect {
        x,
        y,
        width: (right - i64::from(x)) as u32,
        height: (bottom - i64::from(y)) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysicalRect {
        PhysicalRect {
            x,
            y,
            width,
            height,
        }
    }

    fn pending(request: RefreshRequest) -> PendingRefresh {
        let mut pending = PendingRefresh::default();
        pending.push(request);
        pending
    }

    #[test]
    fn absent_requests_keep_the_original_auto_dirty_update() {
        let result = resolve(
            PendingRefresh::default(),
            Some(rect(8, 12, 100, 48)),
            1264,
            1680,
            0,
        )
        .unwrap();
        assert_eq!(
            result,
            ResolvedRefresh {
                region: rect(8, 12, 100, 48),
                waveform: RefreshWaveform::Auto,
                flashing: false
            }
        );
    }

    #[test]
    fn rectangles_are_clamped_and_arithmetic_does_not_wrap() {
        assert_eq!(
            clamp(rect(-20, -40, 100, 120), 1264, 1680),
            Some(rect(0, 0, 80, 80))
        );
        assert_eq!(
            clamp(rect(1200, 1600, u32::MAX, u32::MAX), 1264, 1680),
            Some(rect(1200, 1600, 64, 80))
        );
        assert_eq!(clamp(rect(2000, 2000, 40, 40), 1264, 1680), None);
        assert_eq!(clamp(rect(0, 0, 0, 40), 1264, 1680), None);
    }

    #[test]
    fn an_invalid_request_does_not_refresh_an_unchanged_frame() {
        let request = RefreshRequest {
            waveform: RefreshWaveform::Du,
            flashing: false,
            area: RefreshArea::Rect(rect(2000, 0, 20, 20)),
        };
        assert!(resolve(pending(request), None, 1264, 1680, 0).is_none());
    }

    #[test]
    fn a_full_refresh_is_not_limited_to_the_latest_dirty_rectangle() {
        let result = resolve(
            pending(RefreshRequest::full()),
            Some(rect(8, 12, 100, 48)),
            1264,
            1680,
            0,
        )
        .unwrap();
        assert_eq!(result.region, rect(0, 0, 1264, 1680));
        assert_eq!(result.waveform, RefreshWaveform::Gc16);
        assert!(result.flashing);
    }

    #[test]
    fn an_overlay_update_also_includes_pixels_changed_outside_its_bounds() {
        let request = RefreshRequest {
            waveform: RefreshWaveform::Gc16,
            flashing: true,
            area: RefreshArea::Rect(rect(20, 40, 200, 300)),
        };
        let result = resolve(pending(request), Some(rect(0, 0, 48, 48)), 1264, 1680, 0).unwrap();
        assert_eq!(result.region, rect(0, 0, 220, 340));
    }

    #[test]
    fn requested_rectangles_follow_the_same_rotation_as_pixels() {
        assert_eq!(
            rotate_and_clamp(rect(10, 20, 30, 40), 100, 200, 180),
            Some(rect(60, 140, 30, 40))
        );
        assert_eq!(
            rotate_and_clamp(rect(10, 20, 30, 40), 100, 200, 90),
            Some(rect(40, 10, 40, 30))
        );
        assert_eq!(
            rotate_and_clamp(rect(10, 20, 30, 40), 100, 200, 270),
            Some(rect(20, 160, 40, 30))
        );
    }

    #[test]
    fn mixed_requests_preserve_the_strongest_waveform_and_combined_area() {
        let mut pending = PendingRefresh::default();
        pending.push(RefreshRequest {
            waveform: RefreshWaveform::Du,
            flashing: false,
            area: RefreshArea::Rect(rect(0, 0, 48, 48)),
        });
        pending.push(RefreshRequest {
            waveform: RefreshWaveform::Gc16,
            flashing: false,
            area: RefreshArea::Rect(rect(40, 40, 200, 300)),
        });
        let result = resolve(pending, None, 1264, 1680, 0).unwrap();
        assert_eq!(result.region, rect(0, 0, 240, 340));
        assert_eq!(result.waveform, RefreshWaveform::Gc16);
    }

    #[test]
    fn the_producer_is_thread_safe_and_queue_growth_is_bounded() {
        fn require_send_sync<T: Send + Sync>() {}
        require_send_sync::<RefreshHandle>();
        let wake = crate::wakeup::make_wakeup().unwrap();
        let handle = RefreshHandle {
            pending: Arc::new(Mutex::new(PendingRefresh::default())),
            wake: wake.write,
            quit: Arc::new(AtomicBool::new(false)),
            suspended: Arc::new(AtomicBool::new(false)),
        };
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let handle = handle.clone();
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        handle
                            .request(RefreshRequest {
                                waveform: RefreshWaveform::Auto,
                                flashing: false,
                                area: RefreshArea::Dirty,
                            })
                            .unwrap();
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        let pending = handle.pending.lock().unwrap().take();
        assert_eq!(pending.requests.len(), 1);
        assert_eq!(pending.requests[0], RefreshRequest::full());
        handle.quit.store(true, Ordering::SeqCst);
        assert_eq!(
            handle.request(RefreshRequest::full()),
            Err(RefreshError::EventLoopTerminated)
        );
    }

    #[test]
    fn a_suspended_producer_rejects_refreshes_without_queuing_them() {
        let wake = crate::wakeup::make_wakeup().unwrap();
        let handle = RefreshHandle {
            pending: Arc::new(Mutex::new(PendingRefresh::default())),
            wake: wake.write,
            quit: Arc::new(AtomicBool::new(false)),
            suspended: Arc::new(AtomicBool::new(true)),
        };
        assert_eq!(
            handle.request(RefreshRequest::full()),
            Err(RefreshError::DisplaySuspended)
        );
        assert!(handle.pending.lock().unwrap().is_empty());
        handle.suspended.store(false, Ordering::SeqCst);
        handle.request(RefreshRequest::full()).unwrap();
        assert!(!handle.pending.lock().unwrap().is_empty());
    }
}
