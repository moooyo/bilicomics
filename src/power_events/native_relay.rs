//! Serialize native power records and supervisor barriers through one writer.

use std::{
    fs::File,
    io::{ErrorKind, Read, Write},
    os::{fd::AsRawFd, unix::fs::{FileTypeExt, MetadataExt}},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};

use super::{MAX_NATIVE_POWER_RECORD_BYTES, validate_native_power_record};

const BARRIER_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
struct Packet { bytes: [u8; MAX_NATIVE_POWER_RECORD_BYTES], length: usize }

impl Packet {
    fn content(&self) -> &[u8] { &self.bytes[..self.length] }
}

struct Records {
    input: [u8; MAX_NATIVE_POWER_RECORD_BYTES],
    offset: usize,
    available: usize,
    pending: Packet,
}

enum Received { Packet(Packet), Pending, End }

impl Records {
    fn new() -> Self {
        Self { input: [0; MAX_NATIVE_POWER_RECORD_BYTES], offset: 0, available: 0,
            pending: Packet { bytes: [0; MAX_NATIVE_POWER_RECORD_BYTES], length: 0 } }
    }

    fn partial(&self) -> bool { self.pending.length > 0 }

    fn next(&mut self, input: &mut File) -> Result<Received> {
        loop {
            if self.offset == self.available {
                self.available = match input.read(&mut self.input) {
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(Received::Pending),
                    result => result.context("Read native power relay record")?,
                };
                self.offset = 0;
                if self.available == 0 {
                    ensure!(!self.partial(), "The native power relay stream ended with an incomplete record");
                    return Ok(Received::End);
                }
            }
            while self.offset < self.available {
                let byte = self.input[self.offset];
                self.offset += 1;
                ensure!(self.pending.length < MAX_NATIVE_POWER_RECORD_BYTES - 1 || byte == b'\n', "The native power relay record is too large");
                ensure!(byte == b'\n' || (b' '..=b'~').contains(&byte), "The native power relay record contains unsupported bytes");
                self.pending.bytes[self.pending.length] = byte;
                self.pending.length += 1;
                if byte == b'\n' {
                    let packet = self.pending;
                    self.pending.length = 0;
                    return Ok(Received::Packet(packet));
                }
            }
        }
    }
}

struct Control {
    generation: String,
    token: String,
    startup_seen: bool,
    sequence: u64,
    pending: Option<Packet>,
}

impl Control {
    fn from_environment() -> Result<Self> {
        let generation = std::env::var("BILICOMICS_POWER_RELAY_GENERATION").context("Missing power relay generation")?;
        positive(&generation)?;
        let token = std::env::var("BILICOMICS_POWER_RELAY_TOKEN").context("Missing power relay token")?;
        ensure!(token.len() == 32 && token.bytes().all(|byte| byte.is_ascii_hexdigit()), "Invalid power relay token");
        Ok(Self { generation, token, startup_seen: false, sequence: 0, pending: None })
    }

    fn accept(&mut self, packet: Packet) -> Result<bool> {
        let value = std::str::from_utf8(packet.content())?.strip_suffix('\n').context("Incomplete power relay barrier")?;
        let mut fields = value.split(' ');
        let action = fields.next().context("Missing power relay barrier action")?;
        ensure!(fields.next() == Some(self.generation.as_str()) && fields.next() == Some(self.token.as_str()),
            "The power relay barrier belongs to another launch");
        match action {
            "_bilicomics_start_ready" => {
                ensure!(fields.next().is_none(), "Invalid startup power relay barrier");
                if self.startup_seen || self.sequence > 0 { return Ok(false); }
                self.startup_seen = true;
            }
            "_bilicomics_native_mode_ready" => {
                positive(fields.next().context("Missing power relay sleep generation")?)?;
                let sequence = positive(fields.next().context("Missing power relay capture sequence")?)?;
                ensure!(fields.next().is_none(), "Invalid resume power relay barrier");
                if sequence <= self.sequence { return Ok(false); }
                self.sequence = sequence;
            }
            _ => bail!("Unknown power relay barrier"),
        }
        // A newer wake can replace an invalidated, not-yet-emitted notice. The
        // first pending deadline remains in force instead of being extended.
        self.pending = Some(packet);
        Ok(true)
    }
}

fn positive(value: &str) -> Result<u64> {
    ensure!(!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()), "Invalid power relay sequence");
    let value = value.parse::<u64>()?;
    ensure!(value > 0, "Invalid power relay sequence");
    Ok(value)
}

fn configure(file: &File) -> Result<()> {
    ensure!(file.metadata()?.file_type().is_fifo(), "Native power relay streams must be pipes");
    // SAFETY: the live File owns this descriptor throughout both fcntl calls.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    ensure!(flags >= 0 && unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
        "Configure nonblocking native power relay stream");
    Ok(())
}

fn expired(deadline: Option<Instant>) -> Result<()> {
    ensure!(!deadline.is_some_and(|value| Instant::now() >= value), "The native power relay barrier deadline expired");
    Ok(())
}

fn write_packet(output: &mut File, packet: Packet) -> Result<bool> {
    match output.write(packet.content()) {
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => Ok(false),
        Err(error) => Err(error).context("Forward native power relay record"),
        Ok(written) => {
            ensure!(written == packet.length, "The native power relay record was not written atomically");
            Ok(true)
        }
    }
}

pub(super) fn run(mut source: File, mut commands: File, mut output: File) -> Result<()> {
    let source_identity = source.metadata()?;
    let commands_identity = commands.metadata()?;
    let output_identity = output.metadata()?;
    ensure!((source_identity.dev(), source_identity.ino()) != (commands_identity.dev(), commands_identity.ino())
        && (source_identity.dev(), source_identity.ino()) != (output_identity.dev(), output_identity.ino())
        && (commands_identity.dev(), commands_identity.ino()) != (output_identity.dev(), output_identity.ino()),
        "Native power relay streams must be distinct");
    for file in [&source, &commands, &output] { configure(file)?; }
    let mut control = Control::from_environment()?;
    let mut source_records = Records::new();
    let mut command_records = Records::new();
    let mut native_packet = None;
    let mut deadline = None;
    loop {
        expired(deadline)?;
        // Observe control even while output is back-pressured, so its deadline
        // also covers a full shared FIFO and a continuously readable source.
        loop {
            expired(deadline)?;
            match command_records.next(&mut commands)? {
                Received::Packet(packet) => {
                    if control.accept(packet)? && deadline.is_none() { deadline = Some(Instant::now() + BARRIER_TIMEOUT); }
                }
                Received::Pending => {
                    if command_records.partial() && deadline.is_none() { deadline = Some(Instant::now() + BARRIER_TIMEOUT); }
                    break;
                }
                Received::End => bail!("The native power relay control stream ended unexpectedly"),
            }
        }
        let mut source_drained = false;
        if native_packet.is_none() {
            match source_records.next(&mut source)? {
                Received::Packet(packet) => { validate_native_power_record(packet.content())?; native_packet = Some(packet); }
                Received::Pending => source_drained = true,
                Received::End => {
                    ensure!(control.pending.is_none() && !command_records.partial(), "The native power source ended before its barrier");
                    return Ok(());
                }
            }
        }
        expired(deadline)?;
        if let Some(packet) = native_packet {
            if write_packet(&mut output, packet)? { native_packet = None; continue; }
        } else if source_drained && !source_records.partial() {
            if let Some(packet) = control.pending {
                // This check is repeated after every blocked write. A newly
                // queued native event therefore always precedes the barrier.
                if write_packet(&mut output, packet)? { control.pending = None; }
            }
        }
        if control.pending.is_none() && !command_records.partial() { deadline = None; }
        expired(deadline)?;
        let output_waiting = native_packet.is_some()
            || (control.pending.is_some() && source_drained && !source_records.partial());
        let mut polls = [
            libc::pollfd { fd: commands.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: if native_packet.is_none() { source.as_raw_fd() } else { -1 }, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: if output_waiting { output.as_raw_fd() } else { -1 }, events: libc::POLLOUT, revents: 0 },
        ];
        let timeout = deadline.map_or(-1, |value| value.saturating_duration_since(Instant::now()).as_millis()
            .saturating_add(1).min(i32::MAX as u128) as i32);
        // SAFETY: polls is a live array of valid descriptors or the ignored -1.
        let result = unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, timeout) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == ErrorKind::Interrupted { continue; }
            return Err(error).context("Wait for native power relay progress");
        }
        ensure!(!polls.iter().any(|entry| entry.revents & libc::POLLNVAL != 0), "A native power relay descriptor became invalid");
    }
}
