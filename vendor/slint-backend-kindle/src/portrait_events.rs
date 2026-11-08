//! Non-grabbing native accelerometer event observation.
//!
//! Complete evdev frames notify the caller of a new accepted portrait direction.
//! Current-state metadata resolves queue loss and prevents a stale queued frame
//! from replacing a newer snapshot. This module does not rotate pixels or touch.

use std::{
    fs::File,
    io::{self, Read},
    mem::size_of,
    os::fd::AsRawFd,
};

use crate::{
    NativePortraitOrientation,
    portrait_pose::{
        decode_native_portrait_value, open_native_portrait_sensor, query_native_portrait_value,
    },
};

const EV_SYN: u16 = 0;
const EV_ABS: u16 = 3;
const SYN_REPORT: u16 = 0;
const SYN_DROPPED: u16 = 3;
const ABS_PRESSURE: u16 = 0x18;
const MAX_EVENTS: usize = 4096;
const MAX_READ_CALLS: usize = 4096;
const READ_BATCH_EVENTS: usize = 32;

/// Kernel evdev uses native unsigned-long timestamps, independently of time_t.
/// This is 16 bytes on ARM32 and 24 bytes on Linux x86_64, including musl.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeInputEvent {
    seconds: libc::c_ulong,
    microseconds: libc::c_ulong,
    kind: u16,
    code: u16,
    value: i32,
}

const EVENT_BYTES: usize = size_of::<NativeInputEvent>();

struct EventReader {
    bytes: [u8; EVENT_BYTES * READ_BATCH_EVENTS],
    begin: usize,
    end: usize,
}

impl EventReader {
    fn new() -> Self {
        Self {
            bytes: [0; EVENT_BYTES * READ_BATCH_EVENTS],
            begin: 0,
            end: 0,
        }
    }

    fn has_partial_event(&self) -> bool {
        self.end - self.begin > 0
    }

    fn next_event(
        &mut self,
        file: &File,
        read_calls: &mut usize,
    ) -> io::Result<Option<NativeInputEvent>> {
        loop {
            let remaining = self.end - self.begin;
            if remaining >= EVENT_BYTES {
                // SAFETY: The complete initialized bytes describe a repr(C)
                // event containing only integers. Unaligned reading is allowed.
                let event = unsafe {
                    self.bytes.as_ptr().add(self.begin).cast::<NativeInputEvent>().read_unaligned()
                };
                self.begin += EVENT_BYTES;
                return Ok(Some(event));
            }
            if self.begin > 0 {
                self.bytes.copy_within(self.begin..self.end, 0);
                self.begin = 0;
                self.end = remaining;
            }
            if *read_calls == MAX_READ_CALLS {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "native orientation read budget was exhausted",
                ));
            }
            *read_calls += 1;
            let mut source = file;
            match source.read(&mut self.bytes[self.end..]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        if self.end == 0 {
                            "native orientation source closed"
                        } else {
                            "native orientation source closed during an event"
                        },
                    ));
                }
                Ok(count) => self.end += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(error) => return Err(error),
            }
        }
    }
}

#[derive(Default)]
struct PoseFrames {
    pending: Option<NativePortraitOrientation>,
    dropped: bool,
    needs_snapshot: bool,
}

impl PoseFrames {
    fn observe(
        &mut self,
        event: NativeInputEvent,
        newest_frame: &mut Option<NativePortraitOrientation>,
    ) {
        if event.kind == EV_SYN && event.code == SYN_DROPPED {
            self.pending = None;
            self.dropped = true;
            self.needs_snapshot = false;
            *newest_frame = None;
            return;
        }
        if self.dropped {
            if event.kind == EV_SYN && event.code == SYN_REPORT {
                self.dropped = false;
                self.needs_snapshot = true;
            }
            return;
        }
        // Once a loss boundary ends, all remaining queued records precede the
        // authoritative post-drain snapshot and cannot overwrite it.
        if self.needs_snapshot {
            return;
        }
        if event.kind == EV_ABS && event.code == ABS_PRESSURE {
            if let Some(pose) = decode_native_portrait_value(event.value) {
                self.pending = Some(pose);
            }
        } else if event.kind == EV_SYN && event.code == SYN_REPORT {
            if let Some(pose) = self.pending.take() {
                *newest_frame = Some(pose);
            }
        }
    }
}

pub(crate) struct NativePortraitObserver {
    file: File,
    reader: EventReader,
    frames: PoseFrames,
    current: Option<NativePortraitOrientation>,
    discard_initial_partial: bool,
    #[cfg(test)]
    test_query: Option<Box<dyn FnMut(libc::c_int) -> io::Result<i32>>>,
}

impl NativePortraitObserver {
    /// Open the unique recognized sensor read-only, without EVIOCGRAB.
    /// Discard the old queue before accepting a fresh current-state sample.
    pub(crate) fn open() -> io::Result<Option<Self>> {
        let Some((file, _name)) = open_native_portrait_sensor()? else {
            return Ok(None);
        };
        Self::initialize_with(file, query_native_portrait_value).map(Some)
    }

    fn initialize_with(
        file: File,
        mut query: impl FnMut(libc::c_int) -> io::Result<i32>,
    ) -> io::Result<Self> {
        let mut observer = Self {
            file,
            reader: EventReader::new(),
            frames: PoseFrames::default(),
            current: None,
            discard_initial_partial: false,
            #[cfg(test)]
            test_query: None,
        };
        observer.discard_stale_with_limit(MAX_EVENTS)?;
        // Preserve byte framing if a private fixture's stale event is partial.
        // Its eventual completion remains stale and must not become a new pose.
        observer.discard_initial_partial = observer.reader.has_partial_event();
        observer.current = decode_native_portrait_value(query(observer.fd())?);
        Ok(observer)
    }

    pub(crate) fn fd(&self) -> libc::c_int {
        self.file.as_raw_fd()
    }

    /// Most recent accepted direction; open seeds it from its post-drain sample.
    pub(crate) fn current_pose(&self) -> Option<NativePortraitOrientation> {
        self.current
    }

    fn discard_stale_with_limit(&mut self, maximum: usize) -> io::Result<()> {
        let mut read_calls = 0;
        for count in 0..=maximum {
            match self.reader.next_event(&self.file, &mut read_calls)? {
                None => return Ok(()),
                Some(_) if count == maximum => return Err(queue_limit()),
                Some(_) => {}
            }
        }
        unreachable!("the bounded stale event drain always returns")
    }

    /// Return only a changed, accepted direction after a complete input frame.
    /// Unknown active states preserve the last accepted direction. No events,
    /// duplicate frames and incomplete frames do not request another repaint.
    pub(crate) fn poll(&mut self) -> io::Result<Option<NativePortraitOrientation>> {
        #[cfg(test)]
        if let Some(mut query) = self.test_query.take() {
            let result = self.poll_with_limit(|fd| query(fd), MAX_EVENTS);
            self.test_query = Some(query);
            return result;
        }
        self.poll_with_limit(query_native_portrait_value, MAX_EVENTS)
    }

    #[cfg(test)]
    pub(crate) fn from_test_file(
        file: File,
        current_pose: Option<NativePortraitOrientation>,
        query: impl FnMut(libc::c_int) -> io::Result<i32> + 'static,
    ) -> Self {
        Self {
            file,
            reader: EventReader::new(),
            frames: PoseFrames::default(),
            current: current_pose,
            discard_initial_partial: false,
            test_query: Some(Box::new(query)),
        }
    }

    fn poll_with_limit(
        &mut self,
        mut query: impl FnMut(libc::c_int) -> io::Result<i32>,
        maximum: usize,
    ) -> io::Result<Option<NativePortraitOrientation>> {
        let mut read_calls = 0;
        let mut newest_frame = None;
        for count in 0..=maximum {
            match self.reader.next_event(&self.file, &mut read_calls)? {
                None => break,
                Some(_) if count == maximum => return Err(queue_limit()),
                Some(_) if self.discard_initial_partial => {
                    self.discard_initial_partial = false;
                }
                Some(event) => self.frames.observe(event, &mut newest_frame),
            }
        }
        if self.frames.dropped {
            return Ok(None);
        }
        let candidate = if self.frames.needs_snapshot {
            let current = decode_native_portrait_value(query(self.fd())?);
            self.frames.needs_snapshot = false;
            current
        } else if let Some(frame_pose) = newest_frame {
            let current = decode_native_portrait_value(query(self.fd())?);
            // A newer kernel value may already precede its SYN_REPORT. Wait for
            // that complete frame rather than replaying an older queued pose.
            current.filter(|pose| *pose == frame_pose)
        } else {
            None
        };
        if let Some(pose) = candidate {
            if self.current != Some(pose) {
                self.current = Some(pose);
                return Ok(Some(pose));
            }
        }
        Ok(None)
    }
}

fn queue_limit() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "native orientation queue did not become idle within the event limit",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, io::Write, os::fd::FromRawFd, rc::Rc};

    fn pipe() -> (File, File) {
        let mut descriptors = [-1; 2];
        // SAFETY: The array has room for both newly owned pipe descriptors.
        assert_eq!(
            unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) },
            0,
        );
        // SAFETY: pipe2 transferred unique ownership of both successful FDs.
        unsafe {
            (File::from_raw_fd(descriptors[0]), File::from_raw_fd(descriptors[1]))
        }
    }

    fn event(kind: u16, code: u16, value: i32) -> NativeInputEvent {
        NativeInputEvent {
            kind,
            code,
            value,
            ..NativeInputEvent::default()
        }
    }

    fn pressure(value: i32) -> NativeInputEvent {
        event(EV_ABS, ABS_PRESSURE, value)
    }

    fn report() -> NativeInputEvent {
        event(EV_SYN, SYN_REPORT, 0)
    }

    fn dropped() -> NativeInputEvent {
        event(EV_SYN, SYN_DROPPED, 0)
    }

    fn bytes(events: &[NativeInputEvent]) -> Vec<u8> {
        // SAFETY: repr(C) events consist of two native unsigned longs and eight
        // more initialized bytes. The ABI test below verifies no padding.
        unsafe {
            std::slice::from_raw_parts(
                events.as_ptr().cast::<u8>(),
                std::mem::size_of_val(events),
            )
            .to_vec()
        }
    }

    fn write_events(writer: &mut File, events: &[NativeInputEvent]) {
        writer.write_all(&bytes(events)).unwrap();
    }

    fn observer(initial: i32) -> (NativePortraitObserver, File) {
        let (reader, writer) = pipe();
        let observer = NativePortraitObserver::initialize_with(reader, |_| Ok(initial)).unwrap();
        (observer, writer)
    }

    fn poll_raw(
        observer: &mut NativePortraitObserver,
        raw: i32,
    ) -> io::Result<Option<NativePortraitOrientation>> {
        observer.poll_with_limit(|_| Ok(raw), MAX_EVENTS)
    }

    #[test]
    fn idle_and_non_pose_frames_do_not_query_or_request_a_repaint() {
        let (mut observer, mut writer) = observer(15);
        assert_eq!(observer.current_pose(), Some(NativePortraitOrientation::Up));
        assert_eq!(
            observer.poll_with_limit(|_| panic!("idle input has no pose notification"), MAX_EVENTS).unwrap(),
            None,
        );
        write_events(&mut writer, &[
            pressure(0), report(), event(EV_ABS, 0, 16), report(),
            event(1, ABS_PRESSURE, 16), report(),
        ]);
        assert_eq!(
            observer.poll_with_limit(|_| panic!("non-pose input cannot sample orientation"), MAX_EVENTS).unwrap(),
            None,
        );
        assert_eq!(observer.current_pose(), Some(NativePortraitOrientation::Up));
        write_events(&mut writer, &[pressure(16), report()]);
        assert_eq!(poll_raw(&mut observer, 16).unwrap(), Some(NativePortraitOrientation::Down));
    }

    #[test]
    fn a_complete_batch_coalesces_to_its_latest_valid_direction() {
        let (mut observer, mut writer) = observer(15);
        write_events(&mut writer, &[
            pressure(16), pressure(15), report(), pressure(16), report(),
        ]);
        let mut queries = 0;
        assert_eq!(observer.poll_with_limit(|_| {
            queries += 1;
            Ok(16)
        }, MAX_EVENTS).unwrap(), Some(NativePortraitOrientation::Down));
        assert_eq!(queries, 1);
        assert_eq!(observer.current_pose(), Some(NativePortraitOrientation::Down));
        assert_eq!(poll_raw(&mut observer, 16).unwrap(), None);
    }

    #[test]
    fn duplicate_and_equivalent_portrait_frames_do_not_repaint() {
        let (mut observer, mut writer) = observer(16);
        write_events(&mut writer, &[pressure(16), report(), pressure(18), report()]);
        assert_eq!(poll_raw(&mut observer, 18).unwrap(), None);
        assert_eq!(observer.current_pose(), Some(NativePortraitOrientation::Down));
    }

    #[test]
    fn partial_bytes_and_an_incomplete_syn_frame_are_retained() {
        let (mut observer, mut writer) = observer(15);
        let pose = bytes(&[pressure(16)]);
        let sync = bytes(&[report()]);
        writer.write_all(&pose[..EVENT_BYTES - 1]).unwrap();
        assert_eq!(
            observer.poll_with_limit(|_| panic!("partial event"), MAX_EVENTS).unwrap(),
            None,
        );
        writer.write_all(&pose[EVENT_BYTES - 1..]).unwrap();
        writer.write_all(&sync[..EVENT_BYTES / 2]).unwrap();
        assert_eq!(
            observer.poll_with_limit(|_| panic!("incomplete SYN frame"), MAX_EVENTS).unwrap(),
            None,
        );
        writer.write_all(&sync[EVENT_BYTES / 2..]).unwrap();
        assert_eq!(poll_raw(&mut observer, 16).unwrap(), Some(NativePortraitOrientation::Down));
    }

    #[test]
    fn old_complete_frames_cannot_replace_a_newer_kernel_snapshot() {
        let (mut observer, mut writer) = observer(15);
        write_events(&mut writer, &[pressure(16), report(), pressure(15)]);
        assert_eq!(poll_raw(&mut observer, 15).unwrap(), None);
        assert_eq!(observer.current_pose(), Some(NativePortraitOrientation::Up));
        write_events(&mut writer, &[report()]);
        assert_eq!(poll_raw(&mut observer, 15).unwrap(), None);
        write_events(&mut writer, &[pressure(16), report()]);
        assert_eq!(poll_raw(&mut observer, 16).unwrap(), Some(NativePortraitOrientation::Down));
    }

    #[test]
    fn dropped_frames_wait_for_report_and_use_only_the_fresh_snapshot() {
        let (mut observer, mut writer) = observer(15);
        write_events(&mut writer, &[pressure(16), report(), dropped(), pressure(16)]);
        assert_eq!(
            observer.poll_with_limit(|_| panic!("loss boundary has no report"), MAX_EVENTS).unwrap(),
            None,
        );
        write_events(&mut writer, &[pressure(16), report(), pressure(16), report()]);
        let mut queries = 0;
        assert_eq!(observer.poll_with_limit(|_| {
            queries += 1;
            Ok(15)
        }, MAX_EVENTS).unwrap(), None);
        assert_eq!(queries, 1);
        assert_eq!(observer.current_pose(), Some(NativePortraitOrientation::Up));
        write_events(&mut writer, &[dropped(), pressure(15), report()]);
        assert_eq!(poll_raw(&mut observer, 16).unwrap(), Some(NativePortraitOrientation::Down));
    }

    #[test]
    fn a_non_pose_loss_snapshot_preserves_the_last_accepted_direction() {
        let (mut observer, mut writer) = observer(15);
        write_events(&mut writer, &[dropped(), report()]);
        assert_eq!(poll_raw(&mut observer, 0).unwrap(), None);
        assert_eq!(observer.current_pose(), Some(NativePortraitOrientation::Up));
        write_events(&mut writer, &[pressure(16), report()]);
        assert_eq!(poll_raw(&mut observer, 16).unwrap(), Some(NativePortraitOrientation::Down));
    }

    #[test]
    fn opening_discards_old_events_before_accepting_current_state() {
        let (reader, mut writer) = pipe();
        write_events(&mut writer, &[pressure(16), report()]);
        let mut observer = NativePortraitObserver::initialize_with(reader, |_| Ok(15)).unwrap();
        assert_eq!(observer.current_pose(), Some(NativePortraitOrientation::Up));
        assert_eq!(
            observer.poll_with_limit(|_| panic!("initial stale events were drained"), MAX_EVENTS).unwrap(),
            None,
        );
        write_events(&mut writer, &[pressure(16), report()]);
        assert_eq!(poll_raw(&mut observer, 16).unwrap(), Some(NativePortraitOrientation::Down));
    }

    #[test]
    fn an_initial_partial_event_is_discarded_without_losing_byte_alignment() {
        let (reader, mut writer) = pipe();
        let stale = bytes(&[pressure(16)]);
        writer.write_all(&stale[..EVENT_BYTES / 2]).unwrap();
        let mut observer = NativePortraitObserver::initialize_with(reader, |_| Ok(15)).unwrap();
        writer.write_all(&stale[EVENT_BYTES / 2..]).unwrap();
        write_events(&mut writer, &[report()]);
        assert_eq!(
            observer.poll_with_limit(|_| panic!("completion of an initial stale event"), MAX_EVENTS).unwrap(),
            None,
        );
        write_events(&mut writer, &[pressure(16), report()]);
        assert_eq!(poll_raw(&mut observer, 16).unwrap(), Some(NativePortraitOrientation::Down));
    }

    #[test]
    fn an_unknown_initial_snapshot_waits_for_a_complete_valid_frame() {
        let (mut observer, mut writer) = observer(0);
        assert_eq!(observer.current_pose(), None);
        write_events(&mut writer, &[pressure(15)]);
        assert_eq!(poll_raw(&mut observer, 15).unwrap(), None);
        write_events(&mut writer, &[report()]);
        assert_eq!(poll_raw(&mut observer, 15).unwrap(), Some(NativePortraitOrientation::Up));
    }

    #[test]
    fn event_and_stale_drain_limits_fail_without_committing_a_partial_batch() {
        let (mut observer, mut writer) = observer(15);
        write_events(&mut writer, &[pressure(16), report(), pressure(16), report()]);
        let error = observer.poll_with_limit(|_| panic!("over-budget batch"), 3).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(observer.current_pose(), Some(NativePortraitOrientation::Up));
        write_events(&mut writer, &[pressure(16), report(), pressure(16), report()]);
        assert_eq!(observer.discard_stale_with_limit(3).unwrap_err().kind(), io::ErrorKind::WouldBlock);
    }

    #[test]
    fn closed_and_truncated_sources_return_errors_instead_of_spinning() {
        for prefix in [0, EVENT_BYTES / 2] {
            let (mut observer, mut writer) = observer(15);
            if prefix > 0 {
                writer.write_all(&bytes(&[pressure(16)])[..prefix]).unwrap();
            }
            drop(writer);
            assert_eq!(poll_raw(&mut observer, 16).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
            assert_eq!(observer.current_pose(), Some(NativePortraitOrientation::Up));
        }
    }

    #[test]
    fn injected_pipe_snapshot_uses_the_same_public_poll_path() {
        let (reader, mut writer) = pipe();
        let raw = Rc::new(Cell::new(15));
        let observed_raw = raw.clone();
        let mut observer = NativePortraitObserver::from_test_file(
            reader, Some(NativePortraitOrientation::Up), move |_| Ok(observed_raw.get()),
        );
        write_events(&mut writer, &[pressure(16), report()]);
        raw.set(16);
        assert_eq!(observer.poll().unwrap(), Some(NativePortraitOrientation::Down));
        assert_eq!(observer.poll().unwrap(), None);
    }

    #[test]
    fn native_event_layout_is_independent_of_libc_timeval() {
        let word = size_of::<libc::c_ulong>();
        assert_eq!(EVENT_BYTES, word * 2 + 8);
        assert_eq!(std::mem::offset_of!(NativeInputEvent, kind), word * 2);
        assert_eq!(std::mem::offset_of!(NativeInputEvent, code), word * 2 + 2);
        assert_eq!(std::mem::offset_of!(NativeInputEvent, value), word * 2 + 4);
        assert_eq!(EVENT_BYTES, if word == 4 { 16 } else { 24 });
    }
}
