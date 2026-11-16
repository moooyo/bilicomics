use slint::LogicalPosition;
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::{PointerEventButton, WindowEvent};

use crate::InputObserver;
use crate::input_discovery::{EventSnapshot, MetadataKind};
use std::os::{
    fd::{AsRawFd, IntoRawFd},
    unix::fs::OpenOptionsExt,
};

// Touchscreen driver event. Layout has to match exactly what the kernel writes.
#[repr(C)]
struct TouchInputEvent {
    timestamp_seconds: libc::c_ulong,
    timestamp_microseconds: libc::c_ulong,
    kind: u16,
    code: u16,
    value: i32,
}

// Kernel-defined IDs we match against the `kind` and `code` fields of each event.
const EVENT_SYNC: u16 = 0;
const EVENT_ABSOLUTE_AXIS: u16 = 3;
const EVENT_KEY: u16 = 1; //Amazon "snow protocol", BTN_TOUCH required instead of ABS_MT_TRACKING_ID for some KT4 panels.
const SYNC_REPORT: u16 = 0;
const SYNC_DROPPED: u16 = 3;
const SINGLE_TOUCH_POSITION_X: u16 = 0x00;
const PRESSURE_AXIS: u16 = 0x18;
const TOUCH_SLOT: u16 = 0x2f;
const TOUCH_POSITION_X: u16 = 0x35;
const TOUCH_POSITION_Y: u16 = 0x36;
const TOUCH_TRACKING_ID: u16 = 0x39;
const BTN_TOUCH: u16 = 0x14a;
const MAX_QUEUED_EVENTS: usize = 4096;

// The following constants/functions replicate Linux kernel C macros from <linux/input.h>
// that aren't exposed by the `libc` crate. The formulas expand the _IOW/_IOR macro:
//   _IOC(dir, type, nr, size) = (dir << 30) | (size << 16) | (type << 8) | nr

// EVIOCGRAB: exclusively grabs an input device so no other process receives
// its events. This is essential on Kindle Touch because the zforce touch controller
// is initialized by the framework/X — if we stop the framework, zforce disconnects
// and becomes unusable. Instead, we keep the framework running and grab the device.
// C macro: #define EVIOCGRAB _IOW('E', 0x90, int)
const EVIOCGRAB: libc::c_ulong = 0x40044590;

// C macro: #define EVIOCGABS(abs) _IOR('E', 0x40 + (abs), struct input_absinfo)
// struct input_absinfo is 6 × i32 = 24 bytes
fn eviocgabs(axis: u8) -> libc::c_ulong {
    (2 << 30) | (24 << 16) | ((b'E' as libc::c_ulong) << 8) | (0x40 + axis as libc::c_ulong)
}

// C macro: #define EVIOCGMTSLOTS(len) _IOC(_IOC_READ, 'E', 0x0a, len)
fn eviocgmtslots(bytes: usize) -> libc::c_ulong {
    (2 << 30) | ((bytes as libc::c_ulong) << 16) | ((b'E' as libc::c_ulong) << 8) | 0x0a
}

/// Kernel struct returned by EVIOCGABS.
#[repr(C)]
#[derive(Default)]
struct InputAbsinfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

#[derive(Clone, Copy)]
struct TouchSlot {
    x: i32,
    y: i32,
    tracking_id: i32,
}

struct SlotSnapshot {
    active_slot: i32,
    slots: Vec<TouchSlot>,
}

struct TouchState {
    // The selected Protocol-B slot persists even when no contact is active.
    active_slot: i32,
    // The slot we treat as "the pointer". Slint has no concept of more than one
    // pointer, so we lock onto the first finger down and ignore the others until
    // it lifts. Without this, a second finger's coordinates land in the same
    // x/y and the pointer jitters back and forth between the two fingers.
    tracked_slot: Option<i32>,
    // Protocol-B coordinates persist per slot, including untracked contacts.
    // The kernel does not resend unchanged absolute values for a new contact.
    slots: Option<Vec<TouchSlot>>,
    // Slotless drivers can report coordinates before contact ownership. Keep
    // both axes optional until a fresh synchronization has observed each one.
    legacy_x: Option<i32>,
    legacy_y: Option<i32>,
    x: f32,
    y: f32,
    pressed: bool,
    screen_width: f32,
    screen_height: f32,
    /// Rotation the panel is currently drawn with, handed over on each poll.
    rotation: u32,
    /// Maximum raw X value reported by the touch controller (from EVIOCGABS).
    max_x: f32,
    /// Maximum raw Y value reported by the touch controller (from EVIOCGABS).
    max_y: f32,
}

/// A loss boundary survives every drain and snapshot retry until SYN_REPORT.
#[derive(Default)]
struct ReorientationDrain {
    awaiting_report: bool,
    snapshot_required: bool,
    loss_generation: u64,
}

impl ReorientationDrain {
    fn observe(&mut self, kind: u16, code: u16) {
        if kind == EVENT_SYNC && code == SYNC_DROPPED {
            self.awaiting_report = true;
            self.snapshot_required = true;
            self.loss_generation = self.loss_generation.wrapping_add(1);
        } else if kind == EVENT_SYNC && code == SYNC_REPORT && self.awaiting_report {
            self.awaiting_report = false;
        }
    }

    fn begin_snapshot(&mut self) {
        self.snapshot_required = true;
    }

    fn complete_snapshot(
        &mut self,
        expected_loss_generation: u64,
        events_after_query: usize,
    ) -> bool {
        if self.awaiting_report
            || self.loss_generation != expected_loss_generation
            || events_after_query != 0
        {
            return false;
        }
        self.snapshot_required = false;
        true
    }
}

pub(crate) fn synchronization_retryable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    )
}

pub(crate) struct TouchInput {
    file_descriptor: libc::c_int,
    active: bool,
    state: TouchState,
    reorientation: ReorientationDrain,
}

impl TouchInput {
    pub(crate) fn open(screen_width: u32, screen_height: u32) -> std::io::Result<Self> {
        let file = Self::find_touch_device().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no touch input device found (looked for ABS_MT_POSITION_X in /dev/input/event*)",
            )
        })?;
        // Keep the descriptor whose capabilities were checked during discovery.
        let fd = file.into_raw_fd();

        // Exclusively grab the device so X/framework doesn't consume our events.
        let ret = unsafe { libc::ioctl(fd, EVIOCGRAB as _, 1 as libc::c_int) };
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(err);
        }

        // Query the axis ranges from the device so we scale correctly. On
        // failure we fall back to a common 12-bit max, but log it: a wrong range
        // silently maps touches to the wrong coordinates, which is otherwise
        // baffling to diagnose.
        let max_x = Self::query_axis_max(fd, TOUCH_POSITION_X as u8).unwrap_or_else(|| {
            log::warn!(
                "could not read touch X axis range; assuming max 4095, touch may be miscalibrated"
            );
            4095
        }) as f32;
        let max_y = Self::query_axis_max(fd, TOUCH_POSITION_Y as u8).unwrap_or_else(|| {
            log::warn!(
                "could not read touch Y axis range; assuming max 4095, touch may be miscalibrated"
            );
            4095
        }) as f32;

        // Construct the owner before fallible synchronization so Drop releases
        // both the grab and descriptor if an ioctl or snapshot retry fails.
        let mut input = Self {
            file_descriptor: fd,
            active: false,
            state: TouchState::new(screen_width, screen_height, max_x, max_y),
            reorientation: ReorientationDrain::default(),
        };
        if let Err(error) = input.synchronize_state()
            && !synchronization_retryable(&error)
        {
            return Err(error);
        }
        input.active = true;
        Ok(input)
    }

    fn query_axis_info(fd: libc::c_int, axis: u8) -> std::io::Result<InputAbsinfo> {
        let mut info = InputAbsinfo::default();
        // SAFETY: EVIOCGABS fills this kernel-defined input_absinfo structure.
        let ret = unsafe { libc::ioctl(fd, eviocgabs(axis) as _, &mut info as *mut InputAbsinfo) };
        if ret < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(info)
        }
    }

    /// Query the maximum value for an absolute axis via EVIOCGABS.
    fn query_axis_max(fd: libc::c_int, axis: u8) -> Option<i32> {
        let info = Self::query_axis_info(fd, axis).ok()?;
        // A non-positive maximum would make the scale factor divide by zero
        // (producing inf/NaN coordinates), so treat it as "unknown" and let the
        // caller fall back to a sane default.
        (info.maximum > 0).then_some(info.maximum)
    }

    fn query_abs_bits(fd: libc::c_int) -> std::io::Result<[u8; 8]> {
        let mut bits = [0u8; 8];
        // EVIOCGBIT(EV_ABS, 8), covering all ABS_MT_* codes through 0x3f.
        let request: libc::c_ulong = 2 << 30 | 8 << 16 | (b'E' as libc::c_ulong) << 8 | 0x23;
        // SAFETY: bits has the eight bytes encoded in the ioctl request.
        let result = unsafe { libc::ioctl(fd, request as _, bits.as_mut_ptr()) };
        if result < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(bits)
        }
    }

    fn has_axis(bits: &[u8; 8], axis: u16) -> bool {
        bits[axis as usize / 8] & (1 << (axis % 8)) != 0
    }

    fn query_slot_values(fd: libc::c_int, axis: u16, count: usize) -> std::io::Result<Vec<i32>> {
        // The first word is the requested axis; the remaining words are its
        // per-slot values. A sentinel catches an incomplete successful reply.
        let mut values = vec![i32::MIN; count + 1];
        values[0] = axis as i32;
        let request = eviocgmtslots(values.len() * std::mem::size_of::<i32>());
        // SAFETY: values is an aligned, initialized buffer of the encoded size.
        let result = unsafe { libc::ioctl(fd, request as _, values.as_mut_ptr()) };
        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if values[1..].contains(&i32::MIN) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "touch slot state was incomplete",
            ));
        }
        Ok(values[1..].to_vec())
    }

    fn query_slot_snapshot(fd: libc::c_int) -> std::io::Result<SlotSnapshot> {
        let info = Self::query_axis_info(fd, TOUCH_SLOT as u8)?;
        // The ioctl size field has fourteen bits and includes the axis word.
        let max_count = 0x3fff / std::mem::size_of::<i32>() - 1;
        let count = usize::try_from(info.maximum)
            .ok()
            .and_then(|maximum| maximum.checked_add(1))
            .filter(|&count| count <= max_count);
        let Some(count) =
            count.filter(|_| info.minimum == 0 && info.value >= 0 && info.value <= info.maximum)
        else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "touch slot range or selected slot was invalid",
            ));
        };
        // EVIOCGABS(ABS_MT_POSITION_*) does not return Protocol-B slot values.
        let x = Self::query_slot_values(fd, TOUCH_POSITION_X, count)?;
        let y = Self::query_slot_values(fd, TOUCH_POSITION_Y, count)?;
        let ids = Self::query_slot_values(fd, TOUCH_TRACKING_ID, count)?;
        if ids.iter().any(|&id| id < -1) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "touch slot tracking state was invalid",
            ));
        }
        let slots = x
            .into_iter()
            .zip(y)
            .zip(ids)
            .map(|((x, y), tracking_id)| TouchSlot { x, y, tracking_id })
            .collect();
        Ok(SlotSnapshot {
            active_slot: info.value,
            slots,
        })
    }

    fn synchronize_state(&mut self) -> std::io::Result<()> {
        self.reorientation.begin_snapshot();
        let bits = Self::query_abs_bits(self.file_descriptor)?;
        if !Self::has_axis(&bits, TOUCH_SLOT) {
            self.drain_reorientation()?;
            if self.reorientation.awaiting_report {
                return Err(snapshot_pending());
            }
            self.state.synchronize(None);
            let loss_generation = self.reorientation.loss_generation;
            let after = self.drain_reorientation()?;
            if !self.reorientation.complete_snapshot(loss_generation, after) {
                return Err(snapshot_pending());
            }
            return Ok(());
        }
        if !Self::has_axis(&bits, TOUCH_TRACKING_ID) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Protocol-B touch device did not expose tracking IDs",
            ));
        }
        // A snapshot spans several ioctls. If events arrived during it, drain
        // and retry so older queued deltas cannot overwrite the newer snapshot.
        for _ in 0..8 {
            self.drain_reorientation()?;
            if self.reorientation.awaiting_report {
                return Err(snapshot_pending());
            }
            let loss_generation = self.reorientation.loss_generation;
            let snapshot = Self::query_slot_snapshot(self.file_descriptor)?;
            let after = self.drain_reorientation()?;
            if self.reorientation.complete_snapshot(loss_generation, after) {
                self.state.synchronize(Some(snapshot));
                return Ok(());
            }
            if self.reorientation.awaiting_report {
                return Err(snapshot_pending());
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "touch state kept changing during synchronization",
        ))
    }

    /// Discover a checked owner without closing and reopening the selected node.
    fn find_touch_device() -> Option<std::fs::File> {
        if let Some(snapshot) = EventSnapshot::capture(MetadataKind::AbsoluteAxes, 9) {
            let mut found = None;
            let mut current = true;
            for candidate in snapshot.entries().filter(|entry| {
                entry
                    .axes()
                    .is_some_and(|axes| axes & (1u64 << TOUCH_POSITION_X) != 0)
            }) {
                let file = match candidate.open_checked() {
                    Ok(Some(file)) => file,
                    _ => {
                        current = false;
                        break;
                    }
                };
                let bits = match Self::query_abs_bits(file.as_raw_fd()) {
                    Ok(bits) => bits,
                    Err(_) => {
                        current = false;
                        break;
                    }
                };
                let actual = (0..64).fold(0u64, |axes, axis| {
                    axes | (u64::from(Self::has_axis(&bits, axis)) << axis)
                });
                if candidate.axes() != Some(actual) {
                    current = false;
                    break;
                }
                found = Some(file);
                break;
            }
            if current && found.is_some() && snapshot.is_current() {
                return found;
            }
        }
        for n in 0..10 {
            // Preserve the legacy fallback's handling of linked event nodes.
            let Ok(file) = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
                .open(format!("/dev/input/event{n}"))
            else {
                continue;
            };
            let bits = Self::query_abs_bits(file.as_raw_fd());
            if bits.is_ok_and(|bits| Self::has_axis(&bits, TOUCH_POSITION_X)) {
                return Some(file);
            }
        }
        None
    }

    /// Raw fd of the touch device, for use with `poll(2)` in the event loop.
    pub(crate) fn fd(&self) -> libc::c_int {
        self.file_descriptor
    }

    /// Release stock touch ownership without discarding the native descriptor.
    pub(crate) fn set_active(&mut self, active: bool) -> std::io::Result<()> {
        if self.active == active {
            return Ok(());
        }
        // SAFETY: this is our live evdev descriptor; EVIOCGRAB takes an integer.
        let result =
            unsafe { libc::ioctl(self.file_descriptor, EVIOCGRAB as _, active as libc::c_int) };
        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }
        self.active = false;
        self.state.synchronize(None);
        if active {
            if let Err(error) = self.synchronize_state() {
                if synchronization_retryable(&error) {
                    // Keep the acquired grab while a full-frame input gate
                    // waits for the next complete loss boundary and snapshot.
                    self.active = true;
                    return Ok(());
                }
                // A failed snapshot must never enable zero-coordinate input.
                unsafe { libc::ioctl(self.file_descriptor, EVIOCGRAB as _, 0 as libc::c_int) };
                return Err(error);
            }
        } else {
            self.drain_pending()?;
        }
        self.active = active;
        Ok(())
    }

    /// Cancel a live pointer and discard old contacts without releasing grab.
    /// A consumed observer notification still has to cancel Slint's own grab.
    pub(crate) fn cancel_and_synchronize(
        &mut self,
        window: &MinimalSoftwareWindow,
        observer: &InputObserver,
    ) -> std::io::Result<()> {
        let event = WindowEvent::PointerExited;
        if let Some(callback) = observer.borrow_mut().as_mut() {
            let _ = callback(&event);
        }
        window.try_dispatch_event(event).map_err(|error| {
            std::io::Error::other(format!(
                "cancel native pointer at orientation boundary: {error}"
            ))
        })?;
        self.state.synchronize(None);
        self.synchronize_state()
    }

    /// Bounded draining prevents an unresponsive input source from pinning the UI.
    pub(crate) fn drain_pending(&mut self) -> std::io::Result<()> {
        for _ in 0..MAX_QUEUED_EVENTS {
            if self.read_event()?.is_none() {
                return Ok(());
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "native touch queue did not become idle within the event limit",
        ))
    }

    fn drain_reorientation(&mut self) -> std::io::Result<usize> {
        for count in 0..MAX_QUEUED_EVENTS {
            if self.read_event()?.is_none() {
                return Ok(count);
            }
        }
        Err(snapshot_pending())
    }

    pub(crate) fn snapshot_required(&self) -> bool {
        self.reorientation.snapshot_required
    }

    /// Suppress contacts queued while an orientation frame was being applied.
    pub(crate) fn synchronize_after_frame(&mut self) -> std::io::Result<()> {
        self.synchronize_state()
    }

    /// Read any waiting touch events and forward them to the window as pointer events.
    pub(crate) fn poll(
        &mut self,
        window: &MinimalSoftwareWindow,
        rotation: u32,
        observer: &InputObserver,
    ) -> std::io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.state.rotation = rotation;
        for _ in 0..MAX_QUEUED_EVENTS {
            let Some(event) = self.read_event()? else {
                return Ok(());
            };
            if self.reorientation.snapshot_required {
                return Err(snapshot_pending());
            }
            if let Some(event) =
                self.state
                    .process_event(event.kind, event.code, event.value, window.scale_factor())
            {
                forward_pointer_event(event, observer, |event| {
                    let _ = window.dispatch_event(event);
                });
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "native touch poll budget was exhausted",
        ))
    }

    fn read_event(&mut self) -> std::io::Result<Option<TouchInputEvent>> {
        let mut event = TouchInputEvent {
            timestamp_seconds: 0,
            timestamp_microseconds: 0,
            kind: 0,
            code: 0,
            value: 0,
        };
        let bytes_read = loop {
            // SAFETY: This matching native input_event buffer remains live. A
            // nonblocking evdev read returns complete records or an error.
            let read = unsafe {
                libc::read(
                    self.file_descriptor,
                    &mut event as *mut TouchInputEvent as *mut libc::c_void,
                    std::mem::size_of::<TouchInputEvent>(),
                )
            };
            if read < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
                continue;
            }
            break read;
        };
        if bytes_read == std::mem::size_of::<TouchInputEvent>() as isize {
            self.reorientation.observe(event.kind, event.code);
            return Ok(Some(event));
        }
        if bytes_read < 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(error)
            };
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "native touch returned an incomplete input event",
        ))
    }
}

fn snapshot_pending() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        "native touch requires a complete loss boundary and a fresh slot snapshot",
    )
}

impl TouchState {
    fn new(screen_width: u32, screen_height: u32, max_x: f32, max_y: f32) -> Self {
        Self {
            active_slot: 0,
            tracked_slot: None,
            slots: None,
            legacy_x: None,
            legacy_y: None,
            x: 0.0,
            y: 0.0,
            pressed: false,
            screen_width: screen_width as f32,
            screen_height: screen_height as f32,
            rotation: 0,
            max_x,
            max_y,
        }
    }

    fn synchronize(&mut self, snapshot: Option<SlotSnapshot>) {
        self.active_slot = 0;
        self.tracked_slot = None;
        self.pressed = false;
        self.x = 0.0;
        self.y = 0.0;
        self.slots = None;
        self.legacy_x = None;
        self.legacy_y = None;
        if let Some(snapshot) = snapshot {
            self.active_slot = snapshot.active_slot;
            if let Some(slot) = snapshot.slots.get(snapshot.active_slot as usize) {
                self.x = (slot.x as f32) * self.screen_width / self.max_x;
                self.y = (slot.y as f32) * self.screen_height / self.max_y;
            }
            // Existing IDs are remembered but never adopted as a pointer.
            // A repeated ID stays suppressed until a release or a new ID.
            self.slots = Some(snapshot.slots);
        }
    }

    fn process_event(
        &mut self,
        kind: u16,
        code: u16,
        value: i32,
        scale_factor: f32,
    ) -> Option<WindowEvent> {
        match (kind, code) {
            (EVENT_ABSOLUTE_AXIS, TOUCH_SLOT) => self.active_slot = value,
            (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X | TOUCH_POSITION_Y) => {
                if let Some(slots) = self.slots.as_mut() {
                    let slot = slots.get_mut(self.active_slot as usize)?;
                    if code == TOUCH_POSITION_X {
                        slot.x = value;
                    } else {
                        slot.y = value;
                    }
                } else if code == TOUCH_POSITION_X {
                    self.legacy_x = Some(value);
                } else {
                    self.legacy_y = Some(value);
                }
                if self.tracked_slot == Some(self.active_slot) {
                    if code == TOUCH_POSITION_X {
                        self.x = (value as f32) * self.screen_width / self.max_x;
                    } else {
                        self.y = (value as f32) * self.screen_height / self.max_y;
                    }
                }
            }
            (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID) if self.slots.is_some() => {
                return self.update_tracking_id(value, scale_factor);
            }
            (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID) | (EVENT_KEY, BTN_TOUCH)
                if self.slots.is_none() =>
            {
                // Preserve Type-A and Kindle snow-protocol contact handling.
                // BTN_TOUCH is global and must not select a Protocol-B slot.
                let lifted = (kind == EVENT_ABSOLUTE_AXIS && value == -1)
                    || (kind == EVENT_KEY && value == 0);
                if lifted {
                    if self.tracked_slot == Some(self.active_slot) {
                        self.tracked_slot = None;
                        return self.release(scale_factor);
                    }
                } else if self.tracked_slot.is_none() {
                    self.tracked_slot = Some(self.active_slot);
                }
            }
            (EVENT_SYNC, SYNC_REPORT) => return self.commit(scale_factor),
            _ => {}
        }
        None
    }

    fn update_tracking_id(&mut self, value: i32, scale_factor: f32) -> Option<WindowEvent> {
        let slot = self.slots.as_mut()?.get_mut(self.active_slot as usize)?;
        if value < -1 || slot.tracking_id == value {
            return None;
        }
        slot.tracking_id = value;
        let (x, y) = (slot.x, slot.y);
        if value == -1 {
            if self.tracked_slot == Some(self.active_slot) {
                self.tracked_slot = None;
                return self.release(scale_factor);
            }
            return None;
        }
        // A replacement ID ends the old pointer before beginning a new one.
        // Its release uses the old pointer coordinates, not the new slot data.
        let released = if self.tracked_slot == Some(self.active_slot) {
            self.tracked_slot = None;
            self.release(scale_factor)
        } else {
            None
        };
        if self.tracked_slot.is_none() {
            self.tracked_slot = Some(self.active_slot);
            self.x = (x as f32) * self.screen_width / self.max_x;
            self.y = (y as f32) * self.screen_height / self.max_y;
        }
        released
    }

    fn release(&mut self, scale_factor: f32) -> Option<WindowEvent> {
        if !self.pressed {
            return None;
        }
        self.pressed = false;
        Some(WindowEvent::PointerReleased {
            position: self.logical_position(scale_factor),
            button: PointerEventButton::Left,
        })
    }

    // Called when the driver signals batch of events is finished. Dispatch
    // a press if the finger just touched down, or a move if it was already down.
    fn commit(&mut self, scale_factor: f32) -> Option<WindowEvent> {
        self.tracked_slot?;
        if self.slots.is_none() {
            // Protocol A has no trustworthy initial axis snapshot. Unknown
            // coordinates must not create an origin press or reuse old input.
            self.x = (self.legacy_x? as f32) * self.screen_width / self.max_x;
            self.y = (self.legacy_y? as f32) * self.screen_height / self.max_y;
        }
        let position = self.logical_position(scale_factor);
        let pointer_event = if self.pressed {
            WindowEvent::PointerMoved { position }
        } else {
            self.pressed = true;
            WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            }
        };
        Some(pointer_event)
    }

    /// Calibrated raw evdev uses holding plus app rotation. Compatibility mode
    /// passes only app rotation, preserving the original launch compensation.
    fn logical_position(&self, scale_factor: f32) -> LogicalPosition {
        panel_to_logical_position(
            self.x,
            self.y,
            self.screen_width,
            self.screen_height,
            self.rotation,
            scale_factor,
        )
    }
}

fn forward_pointer_event(
    event: WindowEvent,
    observer: &InputObserver,
    forward: impl FnOnce(WindowEvent),
) {
    let consumed = observer
        .borrow_mut()
        .as_mut()
        .is_some_and(|observer| observer(&event));
    if !consumed {
        forward(event);
    }
}

impl Drop for TouchInput {
    fn drop(&mut self) {
        // Release the exclusive grab before closing
        unsafe { libc::ioctl(self.file_descriptor, EVIOCGRAB as _, 0 as libc::c_int) };
        unsafe { libc::close(self.file_descriptor) };
    }
}

pub(crate) fn has_accelerometer() -> bool {
    std::fs::read_to_string("/proc/bus/input/devices")
        .is_ok_and(|devices| lists_accelerometer(&devices))
}

fn lists_accelerometer(proc_devices: &str) -> bool {
    proc_devices
        .lines()
        .filter_map(|line| line.strip_prefix("B: ABS="))
        .any(|hex_words| {
            let axes = hex_words.split_whitespace().fold(0u64, |axes, word| {
                axes << 32 | u64::from_str_radix(word, 16).unwrap_or(0)
            });
            let reports = |axis: u16| axes & (1 << axis) != 0;
            reports(PRESSURE_AXIS)
                && !reports(SINGLE_TOUCH_POSITION_X)
                && !reports(TOUCH_POSITION_X)
        })
}

/// Slint rotates pixels on the way out but expects pointer events in the
/// un-rotated logical frame, and it exposes no inverse of its rotation, so
/// the mapping from panel pixel back to the app's frame has to live here.
fn undo_rotation(x: f32, y: f32, panel_width: f32, panel_height: f32, rotation: u32) -> (f32, f32) {
    match rotation {
        90 => (y, panel_width - x),
        180 => (panel_width - x, panel_height - y),
        270 => (panel_height - y, x),
        _ => (x, y),
    }
}

/// Touch axes are normalized to panel pixels. Undo rotation in that physical
/// frame before converting to the logical coordinates expected by Slint.
fn panel_to_logical_position(
    x: f32,
    y: f32,
    panel_width: f32,
    panel_height: f32,
    rotation: u32,
    scale_factor: f32,
) -> LogicalPosition {
    let (x, y) = undo_rotation(x, y, panel_width, panel_height, rotation);
    LogicalPosition::new(x / scale_factor, y / scale_factor)
}

#[cfg(test)]
mod tests {
    use super::{
        BTN_TOUCH, EVENT_ABSOLUTE_AXIS, EVENT_KEY, EVENT_SYNC, SYNC_REPORT, SlotSnapshot,
        TOUCH_POSITION_X, TOUCH_POSITION_Y, TOUCH_SLOT, TOUCH_TRACKING_ID, TouchSlot, TouchState,
        forward_pointer_event, lists_accelerometer, panel_to_logical_position, undo_rotation,
    };
    use crate::InputObserver;
    use slint::LogicalPosition;
    use slint::platform::{PointerEventButton, WindowEvent};
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn repeated_loss_boundaries_cannot_authorize_a_slot_snapshot_before_report() {
        let mut drain = super::ReorientationDrain::default();
        drain.begin_snapshot();
        drain.observe(EVENT_SYNC, super::SYNC_DROPPED);
        drain.observe(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X);
        assert!(!drain.complete_snapshot(drain.loss_generation, 0));
        assert!(drain.snapshot_required);
        drain.observe(EVENT_SYNC, SYNC_REPORT);
        let first_snapshot = drain.loss_generation;
        // A second loss after the slot query must survive its post-query drain.
        drain.observe(EVENT_SYNC, super::SYNC_DROPPED);
        assert!(!drain.complete_snapshot(first_snapshot, 1));
        assert!(drain.awaiting_report);
        assert!(drain.snapshot_required);
        drain.observe(EVENT_SYNC, SYNC_REPORT);
        assert!(!drain.complete_snapshot(first_snapshot, 0));
        let fresh_snapshot = drain.loss_generation;
        assert!(drain.complete_snapshot(fresh_snapshot, 0));
        assert!(!drain.snapshot_required);
    }

    #[test]
    fn any_event_during_a_multi_ioctl_slot_query_requires_another_snapshot() {
        let mut drain = super::ReorientationDrain::default();
        drain.begin_snapshot();
        let generation = drain.loss_generation;
        assert!(!drain.complete_snapshot(generation, 1));
        assert!(drain.snapshot_required);
        assert!(drain.complete_snapshot(generation, 0));
    }

    #[test]
    fn raw_touch_uses_holding_plus_app_rotation_before_one_scale_conversion() {
        for (holding, app_rotation, expected) in [
            (
                crate::NativePortraitOrientation::Up,
                0,
                LogicalPosition::new(50.0, 100.0),
            ),
            (
                crate::NativePortraitOrientation::Down,
                0,
                LogicalPosition::new(586.0, 748.0),
            ),
            (
                crate::NativePortraitOrientation::Up,
                90,
                LogicalPosition::new(100.0, 586.0),
            ),
            (
                crate::NativePortraitOrientation::Down,
                90,
                LogicalPosition::new(748.0, 50.0),
            ),
            (
                crate::NativePortraitOrientation::Up,
                270,
                LogicalPosition::new(748.0, 50.0),
            ),
            (
                crate::NativePortraitOrientation::Down,
                270,
                LogicalPosition::new(100.0, 586.0),
            ),
        ] {
            let presentation = crate::portrait::PortraitPresentation {
                holding,
                scanout: crate::NativePortraitOrientation::Up,
            };
            assert_eq!(
                panel_to_logical_position(
                    100.0,
                    200.0,
                    1272.0,
                    1696.0,
                    presentation.touch_rotation(app_rotation),
                    2.0
                ),
                expected
            );
        }
    }

    #[test]
    fn a_flip_snapshot_suppresses_the_held_contact_until_a_new_tracking_id() {
        let mut state = protocol_b_state(0, &[(318, 424, -1)]);
        assert_eq!(
            replay(
                &mut state,
                &[
                    (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 101),
                    (EVENT_SYNC, SYNC_REPORT, 0)
                ]
            )[0]
            .0,
            "press"
        );
        state.synchronize(Some(SlotSnapshot {
            active_slot: 0,
            slots: vec![TouchSlot {
                x: 318,
                y: 424,
                tracking_id: 101,
            }],
        }));
        state.rotation = 180;
        assert!(
            replay(
                &mut state,
                &[
                    (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 101),
                    (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 318),
                    (EVENT_SYNC, SYNC_REPORT, 0),
                    (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                    (EVENT_SYNC, SYNC_REPORT, 0),
                ]
            )
            .is_empty()
        );
        assert_eq!(
            replay(
                &mut state,
                &[
                    (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 102),
                    (EVENT_SYNC, SYNC_REPORT, 0),
                ]
            ),
            vec![("press", LogicalPosition::new(477.0, 636.0))]
        );
    }

    #[test]
    fn native_touch_event_matches_both_supported_linux_word_sizes() {
        let word = std::mem::size_of::<libc::c_ulong>();
        assert_eq!(std::mem::size_of::<super::TouchInputEvent>(), word * 2 + 8);
        assert_eq!(std::mem::offset_of!(super::TouchInputEvent, kind), word * 2);
        assert_eq!(
            std::mem::size_of::<super::TouchInputEvent>(),
            if word == 4 { 16 } else { 24 }
        );
    }

    fn phase(event: &WindowEvent) -> &'static str {
        match event {
            WindowEvent::PointerPressed { .. } => "press",
            WindowEvent::PointerMoved { .. } => "move",
            WindowEvent::PointerReleased { .. } => "release",
            _ => "other",
        }
    }

    fn protocol_b_state(active_slot: i32, slots: &[(i32, i32, i32)]) -> TouchState {
        let mut state = TouchState::new(1272, 1696, 1272.0, 1696.0);
        state.synchronize(Some(SlotSnapshot {
            active_slot,
            slots: slots
                .iter()
                .map(|&(x, y, tracking_id)| TouchSlot { x, y, tracking_id })
                .collect(),
        }));
        state
    }

    fn replay(
        state: &mut TouchState,
        events: &[(u16, u16, i32)],
    ) -> Vec<(&'static str, LogicalPosition)> {
        events
            .iter()
            .filter_map(|&(kind, code, value)| state.process_event(kind, code, value, 2.0))
            .map(|event| match event {
                WindowEvent::PointerPressed { position, .. } => ("press", position),
                WindowEvent::PointerMoved { position } => ("move", position),
                WindowEvent::PointerReleased { position, .. } => ("release", position),
                _ => panic!("unexpected pointer event"),
            })
            .collect()
    }

    #[test]
    fn same_position_protocol_b_tap_uses_synchronized_slot_coordinates() {
        let mut state = protocol_b_state(0, &[(636, 848, -1), (0, 0, -1)]);
        // The observed fts_ts tap contains these seven events and no X/Y/SLOT.
        let output = replay(
            &mut state,
            &[
                (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 0),
                (EVENT_ABSOLUTE_AXIS, 0x30, 33),
                (EVENT_KEY, BTN_TOUCH, 1),
                (EVENT_SYNC, SYNC_REPORT, 0),
                (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                (EVENT_KEY, BTN_TOUCH, 0),
                (EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        let center = LogicalPosition::new(318.0, 424.0);
        assert_eq!(output, [("press", center), ("release", center)]);
    }

    #[test]
    fn global_touch_button_cannot_claim_or_release_a_protocol_b_slot() {
        let mut state = protocol_b_state(1, &[(636, 848, -1), (1000, 1200, -1)]);
        let output = replay(
            &mut state,
            &[
                (EVENT_KEY, BTN_TOUCH, 1),
                (EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 0),
                (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 7),
                (EVENT_SYNC, SYNC_REPORT, 0),
                (EVENT_KEY, BTN_TOUCH, 0),
                (EVENT_SYNC, SYNC_REPORT, 0),
                (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                (EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        let center = LogicalPosition::new(318.0, 424.0);
        assert_eq!(
            output,
            [("press", center), ("move", center), ("release", center)]
        );
    }

    #[test]
    fn untracked_slot_coordinates_survive_until_its_next_contact() {
        let mut state = protocol_b_state(0, &[(100, 200, -1), (0, 0, -1)]);
        let output = replay(
            &mut state,
            &[
                (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 10),
                (EVENT_SYNC, SYNC_REPORT, 0),
                (EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 1),
                (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 11),
                (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 636),
                (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 848),
                (EVENT_SYNC, SYNC_REPORT, 0),
                (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                (EVENT_SYNC, SYNC_REPORT, 0),
                (EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 0),
                (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                (EVENT_SYNC, SYNC_REPORT, 0),
                (EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 1),
                (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 12),
                (EVENT_SYNC, SYNC_REPORT, 0),
                (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                (EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        let first = LogicalPosition::new(50.0, 100.0);
        let center = LogicalPosition::new(318.0, 424.0);
        assert_eq!(
            output,
            [
                ("press", first),
                ("move", first),
                ("move", first),
                ("release", first),
                ("press", center),
                ("release", center),
            ]
        );
    }

    #[test]
    fn resynchronization_suppresses_held_ids_and_restarts_replaced_contacts() {
        let mut state = protocol_b_state(0, &[(636, 848, -1), (0, 0, -1)]);
        assert_eq!(
            replay(
                &mut state,
                &[
                    (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 6),
                    (EVENT_SYNC, SYNC_REPORT, 0),
                ],
            ),
            [("press", LogicalPosition::new(318.0, 424.0))]
        );
        // Resume can find the old finger still down and another slot selected.
        state.synchronize(Some(SlotSnapshot {
            active_slot: 1,
            slots: vec![
                TouchSlot {
                    x: 636,
                    y: 848,
                    tracking_id: 6,
                },
                TouchSlot {
                    x: 0,
                    y: 0,
                    tracking_id: -1,
                },
            ],
        }));
        assert!(
            replay(
                &mut state,
                &[
                    (EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 0),
                    (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 6),
                    (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 700),
                    (EVENT_KEY, BTN_TOUCH, 1),
                    (EVENT_SYNC, SYNC_REPORT, 0),
                    (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                    (EVENT_KEY, BTN_TOUCH, 0),
                    (EVENT_SYNC, SYNC_REPORT, 0),
                ],
            )
            .is_empty()
        );
        let output = replay(
            &mut state,
            &[
                (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 7),
                (EVENT_SYNC, SYNC_REPORT, 0),
                (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 8),
                (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 900),
                (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 1000),
                (EVENT_SYNC, SYNC_REPORT, 0),
                (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                (EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        let resumed = LogicalPosition::new(350.0, 424.0);
        let replaced = LogicalPosition::new(450.0, 500.0);
        assert_eq!(
            output,
            [
                ("press", resumed),
                ("release", resumed),
                ("press", replaced),
                ("release", replaced),
            ]
        );
    }

    #[test]
    fn slotless_tracking_and_snow_protocol_keep_their_contact_fallback() {
        for (kind, code, down, up) in [
            (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 3, -1),
            (EVENT_KEY, BTN_TOUCH, 1, 0),
        ] {
            let mut state = TouchState::new(1272, 1696, 1272.0, 1696.0);
            let output = replay(
                &mut state,
                &[
                    (kind, code, down),
                    (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 636),
                    (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 848),
                    (EVENT_SYNC, SYNC_REPORT, 0),
                    (kind, code, up),
                    (EVENT_SYNC, SYNC_REPORT, 0),
                ],
            );
            let center = LogicalPosition::new(318.0, 424.0);
            assert_eq!(output, [("press", center), ("release", center)]);
        }
    }

    #[test]
    fn slotless_coordinates_precede_contacts_and_survive_unchanged_axes() {
        for (kind, code, down, up) in [
            (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 3, -1),
            (EVENT_KEY, BTN_TOUCH, 1, 0),
        ] {
            let mut state = TouchState::new(1272, 1696, 1272.0, 1696.0);
            let center = LogicalPosition::new(318.0, 424.0);
            let lower = LogicalPosition::new(318.0, 500.0);
            assert_eq!(
                replay(
                    &mut state,
                    &[
                        (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 636),
                        (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 848),
                        (kind, code, down),
                        (EVENT_SYNC, SYNC_REPORT, 0),
                        (kind, code, up),
                        (kind, code, down),
                        (EVENT_SYNC, SYNC_REPORT, 0),
                        (kind, code, up),
                        (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 1000),
                        (kind, code, down),
                        (EVENT_SYNC, SYNC_REPORT, 0),
                        (kind, code, up),
                    ],
                ),
                [
                    ("press", center),
                    ("release", center),
                    ("press", center),
                    ("release", center),
                    ("press", lower),
                    ("release", lower),
                ],
            );
        }
    }

    #[test]
    fn slotless_contact_waits_for_both_coordinates_before_its_first_press() {
        for (kind, code, down, up) in [
            (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 3, -1),
            (EVENT_KEY, BTN_TOUCH, 1, 0),
        ] {
            let mut state = TouchState::new(1272, 1696, 1272.0, 1696.0);
            assert!(
                replay(
                    &mut state,
                    &[
                        (kind, code, down),
                        (EVENT_SYNC, SYNC_REPORT, 0),
                        (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 636),
                        (EVENT_SYNC, SYNC_REPORT, 0),
                    ],
                )
                .is_empty()
            );
            let center = LogicalPosition::new(318.0, 424.0);
            assert_eq!(
                replay(
                    &mut state,
                    &[
                        (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 848),
                        (EVENT_SYNC, SYNC_REPORT, 0),
                        (kind, code, up),
                    ],
                ),
                [("press", center), ("release", center)],
            );
        }
    }

    #[test]
    fn slotless_resynchronization_discards_coordinates_from_before_the_boundary() {
        for (kind, code, down, up) in [
            (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 3, -1),
            (EVENT_KEY, BTN_TOUCH, 1, 0),
        ] {
            let mut state = TouchState::new(1272, 1696, 1272.0, 1696.0);
            replay(
                &mut state,
                &[
                    (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 636),
                    (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 848),
                    (kind, code, down),
                    (EVENT_SYNC, SYNC_REPORT, 0),
                    (kind, code, up),
                ],
            );
            state.synchronize(None);
            assert!(
                replay(
                    &mut state,
                    &[
                        (kind, code, down),
                        (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 1000),
                        (EVENT_SYNC, SYNC_REPORT, 0),
                    ],
                )
                .is_empty()
            );
            let lower = LogicalPosition::new(318.0, 500.0);
            assert_eq!(
                replay(
                    &mut state,
                    &[
                        (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 636),
                        (EVENT_SYNC, SYNC_REPORT, 0),
                        (kind, code, up),
                    ],
                ),
                [("press", lower), ("release", lower)],
            );
        }
    }

    #[test]
    fn input_observer_sees_every_native_phase_and_can_consume_a_swipe() {
        let observed = Rc::new(RefCell::new(Vec::new()));
        let output = observed.clone();
        let observer: InputObserver = Rc::new(RefCell::new(Some(Box::new(move |event| {
            let phase = phase(event);
            output.borrow_mut().push(phase);
            phase != "press"
        }))));
        let mut forwarded = Vec::new();
        for event in [
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(120.0, 180.0),
                button: PointerEventButton::Left,
            },
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(60.0, 180.0),
            },
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(20.0, 180.0),
                button: PointerEventButton::Left,
            },
        ] {
            forward_pointer_event(event, &observer, |event| forwarded.push(phase(&event)));
        }
        assert_eq!(*observed.borrow(), ["press", "move", "release"]);
        assert_eq!(forwarded, ["press"]);
    }

    #[test]
    fn ordinary_native_tap_keeps_its_press_and_release() {
        let observer: InputObserver = Rc::new(RefCell::new(Some(Box::new(|_| false))));
        let mut forwarded = Vec::new();
        for event in [
            WindowEvent::PointerPressed {
                position: LogicalPosition::new(120.0, 180.0),
                button: PointerEventButton::Left,
            },
            WindowEvent::PointerReleased {
                position: LogicalPosition::new(120.0, 180.0),
                button: PointerEventButton::Left,
            },
        ] {
            forward_pointer_event(event, &observer, |event| forwarded.push(phase(&event)));
        }
        assert_eq!(forwarded, ["press", "release"]);
    }

    #[test]
    fn observation_borrow_is_released_before_child_dispatch() {
        let observer: InputObserver = Rc::new(RefCell::new(Some(Box::new(|_| false))));
        forward_pointer_event(
            WindowEvent::PointerMoved {
                position: LogicalPosition::new(120.0, 180.0),
            },
            &observer,
            |_| {
                *observer.borrow_mut() = None;
            },
        );
        assert!(observer.borrow().is_none());
    }

    #[test]
    fn undo_rotation_inverts_slints_rendering_rotation() {
        // Inputs are where Slint's clockwise RenderingRotation puts app pixel
        // (100, 300) on a portrait 1072x1448 panel.
        let (panel_width, panel_height) = (1072.0, 1448.0);
        assert_eq!(
            undo_rotation(100.0, 300.0, panel_width, panel_height, 0),
            (100.0, 300.0)
        );
        assert_eq!(
            undo_rotation(panel_width - 300.0, 100.0, panel_width, panel_height, 90),
            (100.0, 300.0)
        );
        assert_eq!(
            undo_rotation(
                panel_width - 100.0,
                panel_height - 300.0,
                panel_width,
                panel_height,
                180
            ),
            (100.0, 300.0)
        );
        assert_eq!(
            undo_rotation(300.0, panel_height - 100.0, panel_width, panel_height, 270),
            (100.0, 300.0)
        );
    }

    #[test]
    fn panel_coordinates_apply_window_scale_after_rotation() {
        // All four interior vectors represent logical (150, 270) at scale 2.
        // The rotated corners also catch scaling before undoing rotation.
        let (panel_width, panel_height) = (1072.0, 1448.0);
        let vectors = [
            (0, 300.0, 540.0, 150.0, 270.0),
            (90, 532.0, 300.0, 150.0, 270.0),
            (180, 772.0, 908.0, 150.0, 270.0),
            (270, 540.0, 1148.0, 150.0, 270.0),
            (0, 0.0, 0.0, 0.0, 0.0),
            (90, 1072.0, 0.0, 0.0, 0.0),
            (180, 1072.0, 1448.0, 0.0, 0.0),
            (270, 0.0, 1448.0, 0.0, 0.0),
        ];
        for (rotation, x, y, logical_x, logical_y) in vectors {
            assert_eq!(
                panel_to_logical_position(x, y, panel_width, panel_height, rotation, 2.0),
                LogicalPosition::new(logical_x, logical_y),
                "rotation {rotation}, panel position ({x}, {y})"
            );
        }
    }

    #[test]
    fn accelerometer_is_a_pressure_only_device() {
        assert!(lists_accelerometer("B: ABS=1000000\n"));
        // Touchscreen: pressure plus multitouch positions.
        assert!(!lists_accelerometer("B: ABS=260000 1000000\n"));
        // Pen: pressure plus x/y.
        assert!(!lists_accelerometer("B: ABS=1000003\n"));
    }
}
