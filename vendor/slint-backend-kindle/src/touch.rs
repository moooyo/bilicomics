use slint::LogicalPosition;
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::{PointerEventButton, WindowEvent};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

const MAX_INPUT_DIAGNOSTIC_RECORDS: u8 = 16;
static INPUT_DIAGNOSTIC_RECORDS: AtomicU8 = AtomicU8::new(0);
static INPUT_DIAGNOSTIC_RENDER_OFFSET: AtomicU32 = AtomicU32::new(0);
static INPUT_DIAGNOSTIC_NATIVE_TOUCH_OFFSET: AtomicU32 = AtomicU32::new(0);

/// Retain observed render context for opt-in diagnostics, without changing input.
pub(crate) fn set_input_diagnostic_render_offset(offset: u32) {
    INPUT_DIAGNOSTIC_RENDER_OFFSET.store(offset, Ordering::Relaxed);
}

pub(crate) fn set_input_diagnostic_native_touch_offset(offset: u32) {
    INPUT_DIAGNOSTIC_NATIVE_TOUCH_OFFSET.store(offset, Ordering::Relaxed);
}

fn claim_input_diagnostic_record(counter: &AtomicU8) -> Option<u8> {
    let mut count = counter.load(Ordering::Relaxed);
    while count < MAX_INPUT_DIAGNOSTIC_RECORDS {
        match counter.compare_exchange_weak(count, count + 1, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => return Some(count + 1),
            Err(observed) => count = observed,
        }
    }
    None
}

// Touchscreen driver event. Layout has to match exactly what the kernel writes.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TouchInputEvent {
    timestamp_seconds: u32,
    timestamp_microseconds: u32,
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
const SINGLE_TOUCH_POSITION_Y: u16 = 0x01;
const PRESSURE_AXIS: u16 = 0x18;
const TOUCH_SLOT: u16 = 0x2f;
const TOUCH_POSITION_X: u16 = 0x35;
const TOUCH_POSITION_Y: u16 = 0x36;
const TOUCH_TRACKING_ID: u16 = 0x39;
const BTN_TOUCH: u16 = 0x14a;
const MAX_DRAIN_EVENTS: usize = 4096;
const MAX_TOUCH_SLOTS: usize = 256;
const MAX_SNAPSHOT_ATTEMPTS: usize = 8;

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

// EVIOCGMTSLOTS uses one u32 axis code followed by one i32 value per slot.
fn eviocgmtslots(slot_count: usize) -> libc::c_ulong {
    let length = ((slot_count + 1) * std::mem::size_of::<i32>()) as libc::c_ulong;
    (2 << 30) | (length << 16) | ((b'E' as libc::c_ulong) << 8) | 0x0a
}

fn eviocgbit_abs() -> libc::c_ulong {
    (2 << 30) | (8 << 16) | ((b'E' as libc::c_ulong) << 8) | 0x23
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

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct SlotPosition {
    x: Option<i32>,
    y: Option<i32>,
}

impl SlotPosition {
    fn coordinates(self) -> Option<(i32, i32)> {
        Some((self.x?, self.y?))
    }
}

struct DeviceTouchState {
    has_slots: bool,
    has_tracking_ids: bool,
    active_slot: i32,
    positions: Vec<SlotPosition>,
    min_x: i32,
    min_y: i32,
    raw_max_x: i32,
    raw_max_y: i32,
    max_x: f32,
    max_y: f32,
}

impl DeviceTouchState {
    fn query(fd: libc::c_int) -> std::io::Result<Self> {
        Self::query_with(
            TouchInput::supports_axis(fd, TOUCH_SLOT)?,
            TouchInput::supports_axis(fd, TOUCH_TRACKING_ID)?,
            |axis| TouchInput::query_axis_state(fd, axis as u8),
            |axis, slots| TouchInput::query_slot_axis(fd, axis, slots),
        )
    }

    fn query_with(
        has_slots: bool,
        has_tracking_ids: bool,
        mut query_axis: impl FnMut(u16) -> std::io::Result<InputAbsinfo>,
        mut query_slots: impl FnMut(u16, usize) -> std::io::Result<Vec<i32>>,
    ) -> std::io::Result<Self> {
        let x = query_axis(TOUCH_POSITION_X)?;
        let y = query_axis(TOUCH_POSITION_Y)?;
        if x.maximum <= 0 || y.maximum <= 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "touch device reported an unusable position axis range",
            ));
        }
        // EVIOCGABS can return a zero-filled entry for an unsupported axis.
        // Only the capability bitmap can distinguish a legacy MT device.
        let slots = has_slots.then(|| query_axis(TOUCH_SLOT)).transpose()?;
        let (active_slot, positions) = if let Some(slots) = slots {
            if slots.minimum != 0
                || slots.maximum < 0
                || slots.maximum as usize >= MAX_TOUCH_SLOTS
                || slots.value < 0
                || slots.value > slots.maximum
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "touch device reported an invalid current slot or slot range",
                ));
            }
            let count = slots.maximum as usize + 1;
            // MT axis absinfo.value is not the selected slot's position. Linux
            // retains and filters unchanged coordinates in its per-slot array.
            let xs = query_slots(TOUCH_POSITION_X, count)?;
            let ys = query_slots(TOUCH_POSITION_Y, count)?;
            if xs.len() != count || ys.len() != count {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "touch device returned an incomplete slot position snapshot",
                ));
            }
            (
                slots.value,
                xs.into_iter()
                    .zip(ys)
                    .map(|(x, y)| SlotPosition {
                        x: Some(x),
                        y: Some(y),
                    })
                    .collect(),
            )
        } else {
            // Protocol A does not maintain usable MT absinfo.value fields.
            // Wait for observed X and Y instead of inventing an origin press.
            (0, vec![SlotPosition::default()])
        };
        Ok(Self {
            has_slots,
            has_tracking_ids,
            active_slot,
            positions,
            min_x: x.minimum,
            min_y: y.minimum,
            raw_max_x: x.maximum,
            raw_max_y: y.maximum,
            max_x: x.maximum as f32,
            max_y: y.maximum as f32,
        })
    }
}

pub(crate) struct TouchInput {
    file_descriptor: libc::c_int,
    has_slots: bool,
    has_tracking_ids: bool,
    // Protocol-B slot that the incoming ABS_MT_* events currently describe. The
    // driver only emits ABS_MT_SLOT once a second finger appears, so the first
    // contact is implicitly slot 0.
    active_slot: i32,
    // The slot we treat as "the pointer". Slint has no concept of more than one
    // pointer, so we lock onto the first finger down and ignore the others until
    // it lifts. Without this, a second finger's coordinates land in the same
    // x/y and the pointer jitters back and forth between the two fingers.
    tracked_slot: Option<i32>,
    // Coordinates persist independently of contact ownership. Linux may omit
    // either axis, or both axes, when another contact repeats the same point.
    positions: Vec<SlotPosition>,
    pressed: bool,
    screen_width: f32,
    screen_height: f32,
    /// Effective inverse input rotation supplied independently of pixel rotation.
    rotation: u32,
    /// Maximum raw X value reported by the touch controller (from EVIOCGABS).
    max_x: f32,
    /// Maximum raw Y value reported by the touch controller (from EVIOCGABS).
    max_y: f32,
    /// Advertised minima retained only for diagnostics; normalization is unchanged.
    min_x: i32,
    min_y: i32,
    raw_max_x: i32,
    raw_max_y: i32,
    diagnostics_enabled: bool,
    legacy_x_range: Option<(i32, i32)>,
    legacy_y_range: Option<(i32, i32)>,
    legacy_position: SlotPosition,
    legacy_x_event_seen: bool,
    legacy_y_event_seen: bool,
    reorientation_snapshot_required: bool,
    reorientation_discard_until_report: bool,
}

impl TouchInput {
    pub(crate) fn open(screen_width: u32, screen_height: u32) -> std::io::Result<Self> {
        let path = Self::find_touch_device().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no touch input device found (looked for ABS_MT_POSITION_X in /dev/input/event*)",
            )
        })?;
        let c_path = std::ffi::CString::new(path).unwrap();
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }

        // SAFETY: open succeeded and fd has one owner. Closing the lease also
        // releases EVIOCGRAB if a later state query fails before construction.
        let lease = unsafe { OwnedFd::from_raw_fd(fd) };
        // Exclusively grab the device so X/framework doesn't consume our events.
        let ret = unsafe { libc::ioctl(lease.as_raw_fd(), EVIOCGRAB as _, 1 as libc::c_int) };
        if ret < 0 {
            return Err(std::io::Error::last_os_error());
        }

        let state = DeviceTouchState::query(lease.as_raw_fd())?;
        let mut input =
            Self::from_device_state(lease.into_raw_fd(), screen_width, screen_height, state);
        // The fd queue starts before the snapshot. Its implicit-slot prefix
        // must not be interpreted using a newer kernel slot selection.
        input.synchronize_open_with(|| DeviceTouchState::query(fd))?;
        input.capture_legacy_axis_diagnostics();
        Ok(input)
    }

    fn from_device_state(
        fd: libc::c_int,
        screen_width: u32,
        screen_height: u32,
        state: DeviceTouchState,
    ) -> Self {
        Self {
            file_descriptor: fd,
            has_slots: state.has_slots,
            has_tracking_ids: state.has_tracking_ids,
            active_slot: state.active_slot,
            tracked_slot: None,
            positions: state.positions,
            pressed: false,
            screen_width: screen_width as f32,
            screen_height: screen_height as f32,
            rotation: 0,
            max_x: state.max_x,
            max_y: state.max_y,
            min_x: state.min_x,
            min_y: state.min_y,
            raw_max_x: state.raw_max_x,
            raw_max_y: state.raw_max_y,
            diagnostics_enabled: std::env::var("SLINT_KINDLE_INPUT_DIAGNOSTICS")
                .is_ok_and(|value| value == "1"),
            legacy_x_range: None,
            legacy_y_range: None,
            legacy_position: SlotPosition::default(),
            legacy_x_event_seen: false,
            legacy_y_event_seen: false,
            reorientation_snapshot_required: false,
            reorientation_discard_until_report: false,
        }
    }

    fn capture_legacy_axis_diagnostics(&mut self) {
        if !self.diagnostics_enabled {
            return;
        }
        if let Some(info) =
            Self::diagnostic_axis_state(self.file_descriptor, SINGLE_TOUCH_POSITION_X)
        {
            self.legacy_x_range = Some((info.minimum, info.maximum));
            self.legacy_position.x = Some(info.value);
        }
        if let Some(info) =
            Self::diagnostic_axis_state(self.file_descriptor, SINGLE_TOUCH_POSITION_Y)
        {
            self.legacy_y_range = Some((info.minimum, info.maximum));
            self.legacy_position.y = Some(info.value);
        }
    }

    fn diagnostic_axis_state(fd: libc::c_int, axis: u16) -> Option<InputAbsinfo> {
        if !Self::supports_axis(fd, axis).unwrap_or(false) {
            return None;
        }
        // Failed optional diagnostics must never change input initialization.
        Self::query_axis_state(fd, axis as u8).ok()
    }

    fn query_axis_state(fd: libc::c_int, axis: u8) -> std::io::Result<InputAbsinfo> {
        let mut info = InputAbsinfo::default();
        let ret = unsafe { libc::ioctl(fd, eviocgabs(axis) as _, &mut info as *mut InputAbsinfo) };
        if ret < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(info)
        }
    }

    fn supports_axis(fd: libc::c_int, axis: u16) -> std::io::Result<bool> {
        let mut bits = [0u8; 8];
        let index = axis as usize;
        if index >= bits.len() * 8 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "touch axis exceeds the queried capability bitmap",
            ));
        }
        // SAFETY: bits has the eight bytes encoded by this ioctl request.
        let result = unsafe { libc::ioctl(fd, eviocgbit_abs() as _, bits.as_mut_ptr()) };
        if result < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(bits[index / 8] & (1 << (index % 8)) != 0)
        }
    }

    fn query_slot_axis(fd: libc::c_int, axis: u16, count: usize) -> std::io::Result<Vec<i32>> {
        let mut values = vec![0i32; count + 1];
        values[0] = axis as i32;
        // SAFETY: values contains the axis code and space for every bounded
        // slot. The ioctl writes only within that initialized allocation.
        let result = unsafe { libc::ioctl(fd, eviocgmtslots(count) as _, values.as_mut_ptr()) };
        if result < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(values[1..].to_vec())
        }
    }

    /// Scan /dev/input/event* for a device that reports ABS_MT_POSITION_X (0x35).
    fn find_touch_device() -> Option<String> {
        for n in 0..10 {
            let path = format!("/dev/input/event{n}");
            let c_path = std::ffi::CString::new(path.as_str()).unwrap();
            let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
            if fd < 0 {
                continue;
            }
            let supported = Self::supports_axis(fd, TOUCH_POSITION_X).unwrap_or(false);
            unsafe { libc::close(fd) };
            if supported {
                return Some(path);
            }
        }
        None
    }

    /// Raw fd of the touch device, for use with `poll(2)` in the event loop.
    pub(crate) fn fd(&self) -> libc::c_int {
        self.file_descriptor
    }

    /// Cancel Slint's current grab without synthesizing a release or a click.
    /// The application must already reject intents before this is called.
    pub(crate) fn cancel_for_suspend(
        &mut self,
        window: &MinimalSoftwareWindow,
    ) -> Result<(), slint::PlatformError> {
        self.reset_contact_state();
        // Slint 1.17.1 maps PointerExited to MouseEvent::Exit. TouchArea clears
        // pressed/grab state and receives PointerEventKind::Cancel, not Up.
        window.try_dispatch_event(WindowEvent::PointerExited)
    }

    /// Discard pending input and require a new tracking-ID or BTN_TOUCH press.
    /// Call again after the resume repaint and before allowing input polling.
    pub(crate) fn drain_pending(&mut self) -> std::io::Result<()> {
        self.drain_events(true).map(|_| ())
    }

    /// Discard the old coordinate frame before or after a live half-turn.
    /// Queue loss invalidates cached positions until a fresh kernel snapshot.
    pub(crate) fn drain_for_reorientation(&mut self) -> std::io::Result<()> {
        let fd = self.file_descriptor;
        self.drain_for_reorientation_with(|| DeviceTouchState::query(fd))
    }

    fn drain_for_reorientation_with(
        &mut self,
        mut query: impl FnMut() -> std::io::Result<DeviceTouchState>,
    ) -> std::io::Result<()> {
        self.drain_reorientation_events()?;
        self.require_reorientation_report()?;
        if self.reorientation_snapshot_required {
            if self.has_slots {
                for _ in 0..MAX_SNAPSHOT_ATTEMPTS {
                    self.drain_reorientation_events()?;
                    self.require_reorientation_report()?;
                    let state = query()?;
                    if !state.has_slots || state.has_tracking_ids != self.has_tracking_ids {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "touch protocol changed during live resynchronization",
                        ));
                    }
                    let raced = self.drain_reorientation_events()?;
                    self.require_reorientation_report()?;
                    if !raced {
                        self.active_slot = state.active_slot;
                        self.positions = state.positions;
                        self.min_x = state.min_x;
                        self.min_y = state.min_y;
                        self.raw_max_x = state.raw_max_x;
                        self.raw_max_y = state.raw_max_y;
                        self.max_x = state.max_x;
                        self.max_y = state.max_y;
                        self.reorientation_snapshot_required = false;
                        return Ok(());
                    }
                }
                return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock,
                    "live touch input did not remain idle for resynchronization"));
            }
            // Protocol A has no authoritative per-contact MT snapshot.
            // A later genuine press must supply both position axes again.
            self.positions.iter_mut().for_each(|position| *position = SlotPosition::default());
            self.active_slot = 0;
            self.reorientation_snapshot_required = false;
        }
        Ok(())
    }

    fn require_reorientation_report(&self) -> std::io::Result<()> {
        if self.reorientation_discard_until_report {
            Err(std::io::Error::new(std::io::ErrorKind::WouldBlock,
                "live touch queue loss has not reached its synchronization boundary"))
        } else {
            Ok(())
        }
    }

    fn drain_reorientation_events(&mut self) -> std::io::Result<bool> {
        self.reset_contact_state();
        let mut discarded = false;
        for count in 0..=MAX_DRAIN_EVENTS {
            let Some(event) = self.read_event_checked()? else { break; };
            if count == MAX_DRAIN_EVENTS {
                return Err(std::io::Error::other("live touch queue exceeded its drain limit"));
            }
            discarded = true;
            if event.kind == EVENT_SYNC && event.code == SYNC_DROPPED {
                self.reorientation_snapshot_required = true;
                self.reorientation_discard_until_report = true;
                continue;
            }
            if self.reorientation_discard_until_report {
                if event.kind == EVENT_SYNC && event.code == SYNC_REPORT {
                    self.reorientation_discard_until_report = false;
                }
                continue;
            }
            if !self.reorientation_snapshot_required && event.kind == EVENT_ABSOLUTE_AXIS {
                if event.code == TOUCH_SLOT {
                    self.select_slot(event.value)?;
                } else {
                    self.update_position(event.code, event.value);
                }
            }
        }
        Ok(discarded)
    }

    fn synchronize_open_with(
        &mut self,
        mut query_state: impl FnMut() -> std::io::Result<DeviceTouchState>,
    ) -> std::io::Result<()> {
        if !self.has_slots {
            return self.drain_pending();
        }
        for _ in 0..MAX_SNAPSHOT_ATTEMPTS {
            // These events precede our snapshot and have no trustworthy initial
            // slot context. Discard them without applying their position deltas.
            self.drain_events(false)?;
            let state = query_state()?;
            if !state.has_slots || state.has_tracking_ids != self.has_tracking_ids {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "touch protocol capability changed during initialization",
                ));
            }
            // A visible queue race may contain an older implicit-slot prefix.
            // Retry instead of layering it onto newer kernel state. EAGAIN
            // does not make ioctls atomic with a frame awaiting SYN_REPORT.
            if !self.drain_events(false)? {
                self.active_slot = state.active_slot;
                self.positions = state.positions;
                self.min_x = state.min_x;
                self.min_y = state.min_y;
                self.raw_max_x = state.raw_max_x;
                self.raw_max_y = state.raw_max_y;
                self.max_x = state.max_x;
                self.max_y = state.max_y;
                return Ok(());
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "touch input did not remain idle for a position snapshot",
        ))
    }

    fn drain_events(&mut self, retain_positions: bool) -> std::io::Result<bool> {
        self.reset_contact_state();
        let mut discarded = false;
        for count in 0..=MAX_DRAIN_EVENTS {
            match self.read_event_checked()? {
                None => return Ok(discarded),
                Some(_) if count == MAX_DRAIN_EVENTS => {
                    return Err(std::io::Error::other(
                        "touch input queue did not become idle within the drain limit",
                    ));
                }
                Some(event) if retain_positions && event.kind == EVENT_ABSOLUTE_AXIS => {
                    discarded = true;
                    // Discard contact transitions, not persistent slot/axis
                    // deltas: the next press may omit unchanged coordinates.
                    if event.code == TOUCH_SLOT {
                        self.select_slot(event.value)?;
                    } else {
                        self.update_position(event.code, event.value);
                    }
                }
                Some(_) => discarded = true,
            }
        }
        unreachable!("bounded touch drain always returns")
    }

    fn reset_contact_state(&mut self) {
        self.tracked_slot = None;
        self.pressed = false;
    }

    /// Read any waiting touch events and forward them to the window as pointer events.
    pub(crate) fn poll(&mut self, window: &MinimalSoftwareWindow, rotation: u32) {
        if self.reorientation_snapshot_required {
            return;
        }
        self.rotation = rotation;
        while let Some(event) = self.read_event() {
            if self.diagnostics_enabled && event.kind == EVENT_ABSOLUTE_AXIS {
                match event.code {
                    SINGLE_TOUCH_POSITION_X => {
                        self.legacy_position.x = Some(event.value);
                        self.legacy_x_event_seen = true;
                    }
                    SINGLE_TOUCH_POSITION_Y => {
                        self.legacy_position.y = Some(event.value);
                        self.legacy_y_event_seen = true;
                    }
                    _ => {}
                }
            }
            match (event.kind, event.code) {
                (EVENT_ABSOLUTE_AXIS, TOUCH_SLOT) => {
                    if let Err(error) = self.select_slot(event.value) {
                        log::error!("touch input selected an invalid slot: {error}");
                    }
                }
                (EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X | TOUCH_POSITION_Y) => {
                    self.update_position(event.code, event.value);
                }
                (EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID) | (EVENT_KEY, BTN_TOUCH)
                    if !self.has_slots
                        || self.has_tracking_ids == (event.kind == EVENT_ABSOLUTE_AXIS) =>
                {
                    // BTN_TOUCH is global and may precede SLOT. Devices with
                    // slots and tracking IDs must use those IDs for ownership.
                    // Non-slot legacy devices retain their combined BTN/ID
                    // stream; snow-protocol devices without IDs use BTN.
                    let is_lifted = (event.kind == EVENT_ABSOLUTE_AXIS && event.value == -1)
                        || (event.kind == EVENT_KEY && event.value == 0); //Kindle snow protocol - KT4 (https://github.com/koreader/koreader/blob/035e00b21d0286325ef2e8a7e3b6326f340e2ded/frontend/device/kindle/device.lua#L1764)

                    if is_lifted {
                        // The finger in the active slot lifted. Only end the
                        // stroke if it was the one we were following; a lifted
                        // secondary finger must not release the pointer.
                        if self.tracked_slot == Some(self.active_slot) {
                            self.release(window);
                            self.tracked_slot = None;
                        }
                    } else if self.tracked_slot.is_none()
                        && self.position_for_slot(self.active_slot).is_some()
                    {
                        // First finger down owns the pointer until it lifts.
                        self.tracked_slot = Some(self.active_slot);
                    }
                }
                (EVENT_SYNC, SYNC_REPORT) => self.commit(window),
                _ => {}
            }
        }
    }

    fn read_event(&self) -> Option<TouchInputEvent> {
        match self.read_event_checked() {
            Ok(event) => event,
            Err(error) => {
                log::error!("touch input read failed: {error}");
                None
            }
        }
    }

    fn read_event_checked(&self) -> std::io::Result<Option<TouchInputEvent>> {
        loop {
            let mut event = TouchInputEvent::default();
            // SAFETY: event is fully initialized, has the kernel event layout,
            // and remains exclusively borrowed for the read's entire duration.
            let bytes_read = unsafe {
                libc::read(
                    self.file_descriptor,
                    &mut event as *mut TouchInputEvent as *mut libc::c_void,
                    std::mem::size_of::<TouchInputEvent>(),
                )
            };
            if bytes_read == std::mem::size_of::<TouchInputEvent>() as isize {
                return Ok(Some(event));
            }
            if bytes_read < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                if error.kind() == std::io::ErrorKind::WouldBlock {
                    return Ok(None);
                }
                return Err(error);
            }
            return Err(std::io::Error::new(
                if bytes_read == 0 {
                    std::io::ErrorKind::UnexpectedEof
                } else {
                    std::io::ErrorKind::InvalidData
                },
                "touch input returned an incomplete event",
            ));
        }
    }

    fn release(&mut self, window: &MinimalSoftwareWindow) {
        if !self.pressed {
            return;
        }
        self.pressed = false;
        let Some(position) = self.logical_position(window) else {
            return;
        };
        self.record_input_diagnostic("release", window, &position);
        let _ = window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    }

    fn position_for_slot(&self, slot: i32) -> Option<&SlotPosition> {
        usize::try_from(slot)
            .ok()
            .and_then(|index| self.positions.get(index))
    }

    fn select_slot(&mut self, slot: i32) -> std::io::Result<()> {
        if self.position_for_slot(slot).is_none() {
            // Ignore further deltas for this malformed selection; they must
            // never overwrite another slot or start a pointer at its position.
            self.active_slot = -1;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "touch slot exceeds the device's position snapshot",
            ));
        }
        self.active_slot = slot;
        Ok(())
    }

    fn update_position(&mut self, axis: u16, value: i32) {
        let Some(position) = usize::try_from(self.active_slot)
            .ok()
            .and_then(|index| self.positions.get_mut(index))
        else {
            return;
        };
        match axis {
            TOUCH_POSITION_X => position.x = Some(value),
            TOUCH_POSITION_Y => position.y = Some(value),
            _ => {}
        }
    }

    // Called when the driver signals batch of events is finished. Dispatch
    // a press if the finger just touched down, or a move if it was already down.
    fn commit(&mut self, window: &MinimalSoftwareWindow) {
        let Some(position) = self.logical_position(window) else {
            return;
        };
        let pointer_event = if self.pressed {
            WindowEvent::PointerMoved { position }
        } else {
            self.pressed = true;
            self.record_input_diagnostic("press", window, &position);
            WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            }
        };
        let _ = window.dispatch_event(pointer_event);
    }

    fn record_input_diagnostic(
        &self,
        event: &str,
        window: &MinimalSoftwareWindow,
        logical: &LogicalPosition,
    ) {
        if !self.diagnostics_enabled {
            return;
        }
        let Some((raw_x, raw_y)) = self
            .tracked_slot
            .and_then(|slot| self.position_for_slot(slot))
            .and_then(|position| position.coordinates())
        else {
            return;
        };
        // The shared process budget also bounds logs across input reacquisition.
        let Some(sequence) = claim_input_diagnostic_record(&INPUT_DIAGNOSTIC_RECORDS) else {
            return;
        };
        let physical = slint::platform::WindowAdapter::window(window).size();
        let (legacy_min_x, legacy_max_x) = self.legacy_x_range.unwrap_or((0, 0));
        let (legacy_min_y, legacy_max_y) = self.legacy_y_range.unwrap_or((0, 0));
        let native_touch_offset = INPUT_DIAGNOSTIC_NATIVE_TOUCH_OFFSET.load(Ordering::Relaxed);
        let app_rotation = (self.rotation + 360 - native_touch_offset) % 360;
        let _ = writeln!(
            std::io::stderr().lock(),
            "SLINT_KINDLE_INPUT_DIAGNOSTIC seq={} event={} source_x_axis=0x35 source_y_axis=0x36 raw_x={} raw_y={} axis_min_x={} axis_max_x={} axis_min_y={} axis_max_y={} legacy_x_axis=0x00 legacy_x_available={} legacy_x_min={} legacy_x_max={} legacy_y_axis=0x01 legacy_y_available={} legacy_y_min={} legacy_y_max={} legacy_raw_x_available={} legacy_raw_x={} legacy_raw_y_available={} legacy_raw_y={} legacy_x_event_seen={} legacy_y_event_seen={} screen_w={} screen_h={} window_w={} window_h={} window_scale={} app_rotation={} render_offset={} native_touch_offset={} touch_rotation={} normalized_physical_x={} normalized_physical_y={} logical_x={} logical_y={}",
            sequence,
            event,
            raw_x,
            raw_y,
            self.min_x,
            self.raw_max_x,
            self.min_y,
            self.raw_max_y,
            self.legacy_x_range.is_some(),
            legacy_min_x,
            legacy_max_x,
            self.legacy_y_range.is_some(),
            legacy_min_y,
            legacy_max_y,
            self.legacy_position.x.is_some(),
            self.legacy_position.x.unwrap_or(0),
            self.legacy_position.y.is_some(),
            self.legacy_position.y.unwrap_or(0),
            self.legacy_x_event_seen,
            self.legacy_y_event_seen,
            self.screen_width,
            self.screen_height,
            physical.width,
            physical.height,
            window.scale_factor(),
            app_rotation,
            INPUT_DIAGNOSTIC_RENDER_OFFSET.load(Ordering::Relaxed),
            native_touch_offset,
            self.rotation,
            raw_x as f32 * self.screen_width / self.max_x,
            raw_y as f32 * self.screen_height / self.max_y,
            logical.x,
            logical.y,
        );
    }

    /// The owner supplies an inverse input transform separately from pixels.
    /// Legacy input uses app rotation; calibrated raw axes include native portrait.
    fn logical_position(&self, window: &MinimalSoftwareWindow) -> Option<LogicalPosition> {
        let (x, y) = self.position_for_slot(self.tracked_slot?)?.coordinates()?;
        Some(panel_to_logical_position(
            x as f32 * self.screen_width / self.max_x,
            y as f32 * self.screen_height / self.max_y,
            self.screen_width,
            self.screen_height,
            self.rotation,
            window.scale_factor(),
        ))
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

pub(crate) fn lists_accelerometer(proc_devices: &str) -> bool {
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
pub(crate) mod tests {
    use super::{
        BTN_TOUCH, DeviceTouchState, EVENT_ABSOLUTE_AXIS, EVENT_KEY, EVENT_SYNC, InputAbsinfo,
        MAX_SNAPSHOT_ATTEMPTS, MAX_TOUCH_SLOTS, SYNC_REPORT, SlotPosition, TOUCH_POSITION_X,
        TOUCH_POSITION_Y, TOUCH_SLOT, TOUCH_TRACKING_ID, TouchInput, TouchInputEvent,
        claim_input_diagnostic_record, eviocgbit_abs, eviocgmtslots, lists_accelerometer,
        panel_to_logical_position, undo_rotation,
    };
    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
    use slint::{ComponentHandle, LogicalPosition};
    use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
    use std::rc::Rc;

    slint::slint! {
        export component TouchLifecycleTestWindow inherits Window {
            width: 100px;
            height: 100px;
            out property <int> click-count: 0;
            out property <int> cancel-count: 0;
            out property <bool> contact-pressed: contact.pressed;
            contact := TouchArea {
                width: parent.width;
                height: parent.height;
                clicked => { root.click-count += 1; }
                pointer-event(event) => {
                    if event.kind == PointerEventKind.cancel { root.cancel-count += 1; }
                }
            }
        }

        export component TouchPositionTestWindow inherits Window {
            width: 100px;
            height: 100px;
            out property <int> previous-count: 0;
            out property <int> next-count: 0;
            out property <bool> previous-pressed: previous.pressed;
            out property <bool> next-pressed: next.pressed;
            previous := TouchArea {
                x: 0px;
                width: parent.width / 2;
                height: parent.height;
                clicked => { root.previous-count += 1; }
            }
            next := TouchArea {
                x: parent.width / 2;
                width: parent.width / 2;
                height: parent.height;
                clicked => { root.next-count += 1; }
            }
        }

        export component TouchPortraitTestWindow inherits Window {
            width: 100px;
            height: 100px;
            background: #ffffff;
            out property <int> top-left-count: 0;
            out property <int> bottom-right-count: 0;
            out property <bool> top-left-pressed: top.pressed;
            out property <bool> bottom-right-pressed: bottom.pressed;
            out property <int> cancel-count: 0;
            Rectangle {
                x: 0px;
                y: 0px;
                width: parent.width / 2;
                height: parent.height / 2;
                background: #111111;
            }
            Rectangle {
                x: parent.width / 2;
                y: parent.height / 2;
                width: parent.width / 2;
                height: parent.height / 2;
                background: #cccccc;
            }
            top := TouchArea {
                x: 0px;
                y: 0px;
                width: parent.width / 2;
                height: parent.height / 2;
                clicked => { root.top-left-count += 1; }
                pointer-event(event) => {
                    if event.kind == PointerEventKind.cancel { root.cancel-count += 1; }
                }
            }
            bottom := TouchArea {
                x: parent.width / 2;
                y: parent.height / 2;
                width: parent.width / 2;
                height: parent.height / 2;
                clicked => { root.bottom-right-count += 1; }
                pointer-event(event) => {
                    if event.kind == PointerEventKind.cancel { root.cancel-count += 1; }
                }
            }
        }
    }

    struct HeadlessTouchPlatform(Rc<MinimalSoftwareWindow>);

    impl slint::platform::Platform for HeadlessTouchPlatform {
        fn create_window_adapter(
            &self,
        ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }

    fn pipe_input() -> (TouchInput, OwnedFd) {
        pipe_input_with_state(DeviceTouchState {
            has_slots: true,
            has_tracking_ids: true,
            active_slot: 0,
            positions: vec![SlotPosition::default(); 4],
            min_x: 0,
            min_y: 0,
            raw_max_x: 100,
            raw_max_y: 100,
            max_x: 100.0,
            max_y: 100.0,
        })
    }

    fn pipe_input_with_state(state: DeviceTouchState) -> (TouchInput, OwnedFd) {
        let mut descriptors = [0; 2];
        // SAFETY: descriptors has room for the two owned pipe descriptors.
        assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
        // SAFETY: pipe succeeded, so these are distinct owned descriptors.
        let read = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
        let write = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
        let flags = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(read.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
        (
            TouchInput::from_device_state(read.into_raw_fd(), 100, 100, state),
            write,
        )
    }

    fn position_window() -> (Rc<MinimalSoftwareWindow>, TouchPositionTestWindow) {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        slint::platform::set_platform(Box::new(HeadlessTouchPlatform(window.clone()))).unwrap();
        let component = TouchPositionTestWindow::new().unwrap();
        component.show().unwrap();
        window.set_size(slint::PhysicalSize::new(100, 100));
        (window, component)
    }

    pub(crate) fn portrait_window() -> (Rc<MinimalSoftwareWindow>, TouchPortraitTestWindow) {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        slint::platform::set_platform(Box::new(HeadlessTouchPlatform(window.clone()))).unwrap();
        let component = TouchPortraitTestWindow::new().unwrap();
        component.show().unwrap();
        component
            .window()
            .try_dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: 2.0,
            })
            .unwrap();
        window.set_size(slint::PhysicalSize::new(200, 200));
        (window, component)
    }

    pub(crate) fn portrait_input() -> (TouchInput, OwnedFd) {
        let (mut input, writer) = pipe_input();
        input.screen_width = 200.0;
        input.screen_height = 200.0;
        input.diagnostics_enabled = false;
        (input, writer)
    }

    pub(crate) fn queue_portrait_contact(
        writer: &OwnedFd,
        position: Option<(i32, i32)>,
        tracking_id: i32,
    ) {
        let mut events = vec![event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, tracking_id)];
        if let Some((x, y)) = position {
            events.push(event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, x));
            events.push(event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, y));
        }
        events.push(event(EVENT_SYNC, SYNC_REPORT, 0));
        queue_events(writer, &events);
    }

    pub(crate) fn drain_portrait_snapshot(
        input: &mut TouchInput,
        position: (i32, i32),
    ) -> std::io::Result<()> {
        input.drain_for_reorientation_with(|| {
            let mut state = protocol_b_state(1);
            state.positions[1] = SlotPosition { x: Some(position.0), y: Some(position.1) };
            Ok(state)
        })
    }

    pub(crate) fn queue_portrait_sync(writer: &OwnedFd, code: u16) {
        queue_events(writer, &[event(EVENT_SYNC, code, 0)]);
    }

    fn axis_info(value: i32, maximum: i32) -> InputAbsinfo {
        InputAbsinfo {
            value,
            maximum,
            ..InputAbsinfo::default()
        }
    }

    fn protocol_b_state(active_slot: i32) -> DeviceTouchState {
        DeviceTouchState::query_with(
            true,
            true,
            |axis| {
                Ok(match axis {
                    // MT absinfo.value deliberately disagrees with slot 1.
                    TOUCH_POSITION_X | TOUCH_POSITION_Y => axis_info(0, 100),
                    TOUCH_SLOT => axis_info(active_slot, 1),
                    _ => panic!("unexpected axis query"),
                })
            },
            |axis, count| {
                assert_eq!(count, 2);
                Ok(match axis {
                    TOUCH_POSITION_X => vec![10, 86],
                    TOUCH_POSITION_Y => vec![48, 48],
                    _ => panic!("unexpected slot axis query"),
                })
            },
        )
        .unwrap()
    }

    fn event(kind: u16, code: u16, value: i32) -> TouchInputEvent {
        TouchInputEvent {
            kind,
            code,
            value,
            ..TouchInputEvent::default()
        }
    }

    fn queue_events(write: &OwnedFd, events: &[TouchInputEvent]) {
        let bytes = std::mem::size_of_val(events);
        // SAFETY: all event fields are initialized, and the slice outlives write.
        assert_eq!(
            unsafe { libc::write(write.as_raw_fd(), events.as_ptr().cast(), bytes) },
            bytes as isize
        );
    }

    fn queue_portrait_contacts(
        input: &mut TouchInput,
        write: &OwnedFd,
        window: &MinimalSoftwareWindow,
        rotation: u32,
        raw_corners: [(i32, i32); 2],
    ) {
        for (x, y) in raw_corners {
            queue_events(
                write,
                &[
                    event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, x),
                    event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, y),
                    event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 1),
                    event(EVENT_SYNC, SYNC_REPORT, 0),
                    event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                    event(EVENT_SYNC, SYNC_REPORT, 0),
                ],
            );
            input.poll(window, rotation);
        }
    }

    /// Reuse the production raw-input path for another module's portrait mapping.
    pub(crate) fn assert_portrait_corner_hits(
        window: &MinimalSoftwareWindow,
        component: &TouchPortraitTestWindow,
        rotation: u32,
        raw_corners: [(i32, i32); 2],
    ) {
        let before_top = component.get_top_left_count();
        let before_bottom = component.get_bottom_right_count();
        let (mut input, write) = pipe_input();
        input.screen_width = 200.0;
        input.screen_height = 200.0;
        input.diagnostics_enabled = false;
        queue_portrait_contacts(&mut input, &write, window, rotation, raw_corners);
        assert_eq!(component.get_top_left_count(), before_top + 1);
        assert_eq!(component.get_bottom_right_count(), before_bottom + 1);
    }

    #[test]
    fn diagnostic_legacy_coordinates_do_not_feed_the_pointer() {
        let (window, component) = position_window();
        let (mut input, write) = pipe_input_with_state(protocol_b_state(1));
        input.diagnostics_enabled = true;
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, super::SINGLE_TOUCH_POSITION_X, 5),
                event(EVENT_ABSOLUTE_AXIS, super::SINGLE_TOUCH_POSITION_Y, 75),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 1),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert_eq!(input.legacy_position.coordinates(), Some((5, 75)));
        assert!(input.legacy_x_event_seen && input.legacy_y_event_seen);
        assert_eq!(component.get_next_count(), 1);
        assert_eq!(component.get_previous_count(), 0);
        component.hide().unwrap();
    }

    #[test]
    fn input_diagnostic_budget_stays_exhausted_after_sixteen_records() {
        let counter = std::sync::atomic::AtomicU8::new(0);
        for sequence in 1..=16 {
            assert_eq!(claim_input_diagnostic_record(&counter), Some(sequence));
        }
        assert_eq!(claim_input_diagnostic_record(&counter), None);
        assert_eq!(claim_input_diagnostic_record(&counter), None);
    }

    #[test]
    fn calibrated_portrait_touch_reaches_both_corners_at_one_scale() {
        let (window, component) = portrait_window();
        let (mut input, write) = pipe_input();
        input.screen_width = 200.0;
        input.screen_height = 200.0;
        input.diagnostics_enabled = false;
        // Raw fixed-panel vectors for logical (20,20) and (80,80). Each row
        // combines app rotation with the independent native portrait half-turn.
        let vectors = [
            (0, 0, (20, 20), (80, 80)),
            (0, 180, (80, 80), (20, 20)),
            (90, 0, (80, 20), (20, 80)),
            (90, 180, (20, 80), (80, 20)),
            (180, 0, (80, 80), (20, 20)),
            (180, 180, (20, 20), (80, 80)),
            (270, 0, (20, 80), (80, 20)),
            (270, 180, (80, 20), (20, 80)),
        ];
        for (index, (app_rotation, native_offset, top, bottom)) in vectors.into_iter().enumerate() {
            queue_portrait_contacts(
                &mut input,
                &write,
                &window,
                (app_rotation + native_offset) % 360,
                [top, bottom],
            );
            assert_eq!(
                component.get_top_left_count(),
                (index + 1) as i32,
                "app_rotation={app_rotation}, native_offset={native_offset}"
            );
            assert_eq!(
                component.get_bottom_right_count(),
                (index + 1) as i32,
                "app_rotation={app_rotation}, native_offset={native_offset}"
            );
        }
        component.hide().unwrap();
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

    #[test]
    fn reopened_slot_reuses_unchanged_axes_and_keeps_secondary_positions_separate() {
        let (window, component) = position_window();
        let (mut input, write) = pipe_input_with_state(protocol_b_state(1));
        assert_eq!(input.positions[1].coordinates(), Some((86, 48)));
        assert_eq!(input.tracked_slot, None);
        assert!(!input.pressed);

        // The reopened device remains on slot 1. Its repeated right-side tap
        // has neither a SLOT event nor either unchanged position axis.
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 1),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert_eq!(component.get_next_count(), 1);
        assert_eq!(component.get_previous_count(), 0);

        // A single changed axis must retain the other coordinate from the
        // device snapshot, rather than turning a next tap into previous.
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 2),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 55),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert_eq!(input.positions[1].coordinates(), Some((86, 55)));
        assert!(component.get_next_pressed());

        // A secondary contact updates its own cache without moving or
        // releasing the first pointer. Its later fresh tap may omit both axes.
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 0),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 3),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 20),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 70),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert!(component.get_next_pressed());
        assert!(!component.get_previous_pressed());
        assert_eq!(component.get_next_count(), 1);
        assert_eq!(component.get_previous_count(), 0);
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 1),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 0),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 4),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert_eq!(component.get_next_count(), 2);
        assert_eq!(component.get_previous_count(), 1);
        component.hide().unwrap();
    }

    #[test]
    fn global_btn_cannot_claim_a_stale_slot_before_its_tracking_id() {
        let (window, component) = position_window();
        let (mut input, write) = pipe_input_with_state(protocol_b_state(1));
        queue_events(
            &write,
            &[
                // This global key precedes the real contact's slot selection.
                event(EVENT_KEY, BTN_TOUCH, 1),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 0),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 5),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert_eq!(input.tracked_slot, Some(0));
        assert!(component.get_previous_pressed());
        assert!(!component.get_next_pressed());
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                event(EVENT_KEY, BTN_TOUCH, 0),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert_eq!(component.get_previous_count(), 1);
        assert_eq!(component.get_next_count(), 0);
        assert_eq!(input.tracked_slot, None);
        assert!(!input.pressed);
        assert!(!component.get_previous_pressed());
        component.hide().unwrap();
    }

    #[test]
    fn drained_wake_coordinates_survive_without_replaying_the_wake_contact() {
        let (window, component) = position_window();
        let (mut input, write) = pipe_input_with_state(protocol_b_state(0));
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 5),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 86),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 48),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.drain_pending().unwrap();
        assert_eq!(input.positions[0].coordinates(), Some((86, 48)));
        assert_eq!(input.tracked_slot, None);
        assert!(!input.pressed);

        // The tail of the discarded gesture is not a new press, but its
        // position changes must still be remembered for the next fresh ID.
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 52),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert_eq!(component.get_next_count(), 0);
        assert_eq!(component.get_previous_count(), 0);
        assert!(!component.get_next_pressed());
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 6),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert_eq!(component.get_next_count(), 1);
        assert_eq!(component.get_previous_count(), 0);
        assert_eq!(input.positions[0].coordinates(), Some((86, 52)));
        component.hide().unwrap();
    }

    #[test]
    fn opening_discards_older_implicit_slot_prefix_and_retries_snapshot_races() {
        let (window, component) = position_window();
        let (mut input, write) = pipe_input_with_state(protocol_b_state(1));
        // The fd opened on slot 0, but its later kernel snapshot already names
        // slot 1. Applying this old prefix to that newer slot would corrupt X1.
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 20),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 48),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 1),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 7),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        let mut queries = 0;
        input
            .synchronize_open_with(|| {
                queries += 1;
                let mut state = protocol_b_state(1);
                state.positions[0].x = Some(if queries == 1 { 20 } else { 25 });
                if queries == 1 {
                    // This frame races the first query. It must cause a fresh
                    // snapshot, not be interpreted on top of that snapshot.
                    queue_events(
                        &write,
                        &[
                            event(EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 0),
                            event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 25),
                            event(EVENT_SYNC, SYNC_REPORT, 0),
                            event(EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 1),
                            event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 8),
                            event(EVENT_SYNC, SYNC_REPORT, 0),
                        ],
                    );
                }
                Ok(state)
            })
            .unwrap();
        assert_eq!(queries, 2);
        assert_eq!(input.active_slot, 1);
        assert_eq!(input.positions[0].coordinates(), Some((25, 48)));
        assert_eq!(input.positions[1].coordinates(), Some((86, 48)));
        assert_eq!(input.tracked_slot, None);
        assert!(!input.pressed);
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 9),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert_eq!(component.get_next_count(), 1);
        assert_eq!(component.get_previous_count(), 0);
        component.hide().unwrap();
    }

    #[test]
    fn legacy_contact_waits_for_observed_coordinates_instead_of_trusting_mt_absinfo() {
        let state = DeviceTouchState::query_with(
            false,
            false,
            |axis| match axis {
                TOUCH_POSITION_X | TOUCH_POSITION_Y => Ok(axis_info(10, 100)),
                // Linux can return this zero entry even without SLOT support.
                TOUCH_SLOT => Ok(axis_info(0, 0)),
                _ => panic!("unexpected axis query"),
            },
            |_, _| panic!("single-contact input has no slot query"),
        )
        .unwrap();
        assert_eq!(state.positions[0].coordinates(), None);
        let (window, component) = position_window();
        let (mut input, write) = pipe_input_with_state(state);
        queue_events(
            &write,
            &[
                event(EVENT_KEY, BTN_TOUCH, 1),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_KEY, BTN_TOUCH, 0),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_KEY, BTN_TOUCH, 1),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 86),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert!(!component.get_next_pressed());
        assert!(!component.get_previous_pressed());
        assert_eq!(component.get_previous_count(), 0);
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 48),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_KEY, BTN_TOUCH, 0),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_KEY, BTN_TOUCH, 1),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 58),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_KEY, BTN_TOUCH, 0),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert_eq!(component.get_next_count(), 2);
        assert_eq!(component.get_previous_count(), 0);
        assert_eq!(input.positions[0].coordinates(), Some((86, 58)));
        component.hide().unwrap();
    }

    #[test]
    fn non_slot_tracking_capability_retains_legacy_btn_release() {
        let state = DeviceTouchState::query_with(
            false,
            true,
            |axis| match axis {
                TOUCH_POSITION_X | TOUCH_POSITION_Y => Ok(axis_info(0, 100)),
                _ => panic!("a non-slot device must not query slot state"),
            },
            |_, _| panic!("a non-slot device has no slot query"),
        )
        .unwrap();
        let (window, component) = position_window();
        let (mut input, write) = pipe_input_with_state(state);
        queue_events(
            &write,
            &[
                event(EVENT_KEY, BTN_TOUCH, 1),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 10),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 86),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 48),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert!(component.get_next_pressed());
        queue_events(
            &write,
            &[
                // This legacy stream releases through BTN, without ID -1.
                event(EVENT_KEY, BTN_TOUCH, 0),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert_eq!(component.get_next_count(), 1);
        assert_eq!(component.get_previous_count(), 0);
        assert_eq!(input.tracked_slot, None);
        assert!(!input.pressed);
        assert!(!component.get_next_pressed());
        component.hide().unwrap();
    }

    #[test]
    fn slot_snapshots_require_valid_ranges_complete_values_and_successful_ioctls() {
        assert_eq!(eviocgmtslots(2), 0x800c450a);
        assert_eq!(eviocgbit_abs(), 0x80084523);
        for invalid in [
            axis_info(-1, 1),
            axis_info(2, 1),
            axis_info(0, MAX_TOUCH_SLOTS as i32),
        ] {
            let result = DeviceTouchState::query_with(
                true,
                true,
                |axis| {
                    Ok(if axis == TOUCH_SLOT {
                        axis_info(invalid.value, invalid.maximum)
                    } else {
                        axis_info(0, 100)
                    })
                },
                |_, _| panic!("invalid slot ranges must not issue slot ioctls"),
            );
            assert!(result.is_err());
        }
        for error in [libc::EIO, libc::EINVAL, libc::ENOTTY] {
            let result = DeviceTouchState::query_with(
                true,
                true,
                |axis| Ok(axis_info(0, if axis == TOUCH_SLOT { 1 } else { 100 })),
                |_, _| Err(std::io::Error::from_raw_os_error(error)),
            );
            assert_eq!(result.err().unwrap().raw_os_error(), Some(error));
        }
        assert!(
            DeviceTouchState::query_with(
                true,
                true,
                |axis| Ok(axis_info(0, if axis == TOUCH_SLOT { 1 } else { 100 })),
                |_, _| Ok(vec![86]),
            )
            .is_err()
        );
        let result = DeviceTouchState::query_with(
            true,
            true,
            |axis| {
                if axis == TOUCH_SLOT {
                    Err(std::io::Error::from_raw_os_error(libc::ENODEV))
                } else {
                    Ok(axis_info(0, 100))
                }
            },
            |_, _| panic!("unavailable input must not fall back to a position"),
        );
        assert_eq!(result.err().unwrap().raw_os_error(), Some(libc::ENODEV));

        let (mut input, write) = pipe_input_with_state(protocol_b_state(0));
        assert!(input.select_slot(-1).is_err());
        input.update_position(TOUCH_POSITION_X, 99);
        assert_eq!(input.positions[0].coordinates(), Some((10, 48)));
        queue_events(&write, &[event(EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 2)]);
        assert_eq!(
            input.drain_pending().unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );

        let (mut input, write) = pipe_input_with_state(protocol_b_state(0));
        let mut queries = 0;
        let result = input.synchronize_open_with(|| {
            queries += 1;
            queue_events(&write, &[event(EVENT_SYNC, SYNC_REPORT, 0)]);
            Ok(protocol_b_state(0))
        });
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(queries, MAX_SNAPSHOT_ATTEMPTS);
        assert_eq!(input.tracked_slot, None);
        assert!(!input.pressed);
    }

    #[test]
    fn suspended_contact_cancels_without_click_and_requires_a_new_sequence() {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        slint::platform::set_platform(Box::new(HeadlessTouchPlatform(window.clone()))).unwrap();
        let component = TouchLifecycleTestWindow::new().unwrap();
        component.show().unwrap();
        window.set_size(slint::PhysicalSize::new(100, 100));
        let (mut input, write) = pipe_input();
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 1),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 50),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 50),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert!(component.get_contact_pressed());

        input.cancel_for_suspend(&window).unwrap();
        assert!(!component.get_contact_pressed());
        assert_eq!(component.get_cancel_count(), 1);
        assert_eq!(component.get_click_count(), 0);

        // A queued wake gesture is discarded without forwarding any events.
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 2),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 60),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 60),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.drain_pending().unwrap();
        // Remaining coordinates, sync and release from that old contact cannot
        // become a new press after the queue reset.
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 70),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert!(!component.get_contact_pressed());
        assert_eq!(component.get_click_count(), 0);

        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 3),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 50),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 50),
                event(EVENT_SYNC, SYNC_REPORT, 0),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert_eq!(component.get_click_count(), 1);
        assert!(!component.get_contact_pressed());

        // Discarding a gesture must retain its selected protocol-B slot. A
        // later tracking ID need not be preceded by another SLOT event.
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 1),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 4),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.drain_pending().unwrap();
        assert_eq!(input.active_slot, 1);
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 5),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 50),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_Y, 50),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert_eq!(input.tracked_slot, Some(1));
        assert!(component.get_contact_pressed());
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 0),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert!(component.get_contact_pressed());
        assert_eq!(component.get_click_count(), 1);
        queue_events(
            &write,
            &[
                event(EVENT_ABSOLUTE_AXIS, TOUCH_SLOT, 1),
                event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
                event(EVENT_SYNC, SYNC_REPORT, 0),
            ],
        );
        input.poll(&window, 0);
        assert!(!component.get_contact_pressed());
        assert_eq!(component.get_click_count(), 2);
        component.hide().unwrap();
    }

    #[test]
    fn drain_resets_local_contact_and_reports_a_closed_or_partial_stream() {
        let (mut input, write) = pipe_input();
        assert!(DeviceTouchState::query(-1).is_err());
        input.tracked_slot = Some(2);
        input.active_slot = 2;
        input.pressed = true;
        queue_events(&write, &[event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 4)]);
        input.drain_pending().unwrap();
        assert_eq!(input.tracked_slot, None);
        assert_eq!(input.active_slot, 2);
        assert!(!input.pressed);
        let one_byte = [1u8];
        assert_eq!(
            unsafe { libc::write(write.as_raw_fd(), one_byte.as_ptr().cast(), one_byte.len()) },
            1
        );
        assert_eq!(
            input.drain_pending().unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        drop(write);
        assert_eq!(
            input.drain_pending().unwrap_err().kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn live_reorientation_loss_uses_fresh_positions_and_requires_a_new_contact() {
        let (window, component) = position_window();
        let (mut input, write) = pipe_input_with_state(protocol_b_state(0));
        queue_events(&write, &[
            event(EVENT_SYNC, super::SYNC_DROPPED, 0),
            event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 3),
            event(EVENT_SYNC, SYNC_REPORT, 0),
        ]);
        input.drain_for_reorientation_with(|| Ok(protocol_b_state(1))).unwrap();
        assert_eq!(input.active_slot, 1);
        assert!(!input.reorientation_snapshot_required);
        queue_events(&write, &[
            event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 22),
            event(EVENT_SYNC, SYNC_REPORT, 0),
            event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
            event(EVENT_SYNC, SYNC_REPORT, 0),
        ]);
        input.poll(&window, 0);
        assert_eq!(component.get_next_count(), 1);
        assert_eq!(component.get_previous_count(), 0);
        component.hide().unwrap();
    }

    #[test]
    fn live_reorientation_snapshot_failure_keeps_input_gated_until_recovery() {
        let (window, component) = position_window();
        let (mut input, write) = pipe_input_with_state(protocol_b_state(0));
        queue_events(&write, &[
            event(EVENT_SYNC, super::SYNC_DROPPED, 0),
            event(EVENT_SYNC, SYNC_REPORT, 0),
        ]);
        assert!(input.drain_for_reorientation_with(|| Err(std::io::Error::other("fixture failure"))).is_err());
        assert!(input.reorientation_snapshot_required);
        queue_events(&write, &[
            event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 23),
            event(EVENT_ABSOLUTE_AXIS, TOUCH_POSITION_X, 3),
            event(EVENT_SYNC, SYNC_REPORT, 0),
        ]);
        input.poll(&window, 0);
        assert_eq!(component.get_next_count(), 0);
        assert_eq!(component.get_previous_count(), 0);
        input.drain_for_reorientation_with(|| Ok(protocol_b_state(1))).unwrap();
        assert!(!input.reorientation_snapshot_required);
        queue_events(&write, &[
            event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, 24),
            event(EVENT_SYNC, SYNC_REPORT, 0),
            event(EVENT_ABSOLUTE_AXIS, TOUCH_TRACKING_ID, -1),
            event(EVENT_SYNC, SYNC_REPORT, 0),
        ]);
        input.poll(&window, 0);
        assert_eq!(component.get_next_count(), 1);
        assert_eq!(component.get_previous_count(), 0);
        component.hide().unwrap();
    }

    #[test]
    fn live_reorientation_waits_for_the_loss_report_without_querying_early() {
        let (mut input, write) = pipe_input_with_state(protocol_b_state(0));
        queue_events(&write, &[event(EVENT_SYNC, super::SYNC_DROPPED, 0)]);
        assert_eq!(input.drain_for_reorientation_with(|| panic!("loss boundary is incomplete"))
            .unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        assert!(input.reorientation_snapshot_required);
        queue_events(&write, &[event(EVENT_SYNC, SYNC_REPORT, 0)]);
        input.drain_for_reorientation_with(|| Ok(protocol_b_state(1))).unwrap();
        assert_eq!(input.active_slot, 1);
        assert!(!input.reorientation_snapshot_required);
    }

    #[test]
    fn a_second_loss_during_snapshot_keeps_the_report_boundary_closed() {
        let (mut input, write) = pipe_input_with_state(protocol_b_state(0));
        queue_events(&write, &[
            event(EVENT_SYNC, super::SYNC_DROPPED, 0),
            event(EVENT_SYNC, SYNC_REPORT, 0),
        ]);
        let error = input.drain_for_reorientation_with(|| {
            queue_events(&write, &[event(EVENT_SYNC, super::SYNC_DROPPED, 0)]);
            Ok(protocol_b_state(1))
        }).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert!(input.reorientation_snapshot_required);
        assert!(input.reorientation_discard_until_report);
        assert_eq!(input.active_slot, 0);
        assert!(input.drain_for_reorientation_with(|| panic!("the next report is absent")).is_err());
        queue_events(&write, &[event(EVENT_SYNC, SYNC_REPORT, 0)]);
        input.drain_for_reorientation_with(|| Ok(protocol_b_state(1))).unwrap();
        assert_eq!(input.active_slot, 1);
        assert!(!input.reorientation_snapshot_required);
    }
}
