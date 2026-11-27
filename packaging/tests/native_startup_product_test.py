#!/usr/bin/python3 -IB
"""Exercise the complete launcher and Kindle-feature host ELF, not the UI.

Inside a fresh root-owned 0700 ubuntu-run workspace with a separate PID
namespace and loopback-only networking, stage the candidate at /work/app
and its dynamically linked x86-64 Kindle-feature product at
/work/artifacts/host-bilicomics. From /work, run this file with
/usr/bin/python3 -IB. The fixture needs the Ubuntu C compiler and pidfds.

It mounts synthetic hardware and a private /tmp only inside this jail.
No launcher text, protocol implementation, PID data or clock is replaced.
The real relay receives synthetic LIPC source events. This covers the
pre-App handoff, not Library admission, active UI, resume or hardware.
"""

from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
import ctypes
import errno
import json
import os
from pathlib import Path
import re
import select
import shutil
import signal
import socket
import stat
import struct
import subprocess
import sys
import time


WORK = Path("/work")
APP = WORK / "app"
ELF = WORK / "artifacts/host-bilicomics"
FIXTURE = APP / "docs/verification/startup-display-sampling-20261122/product_fixture"
LAUNCH = APP / "packaging/kindle/launch.sh"
LOCK = Path("/tmp/bilicomics-launcher.lock")
GRANT = LOCK / "native-startup-grant"
INVENTORY = Path("/proc/bus/input/devices")
FRAMEBUFFER = Path("/dev/fb0")
FIRMWARE = "5.20.1"
EXITED = {"Z", "X", "x"}
DISPLAY_QUERIES = {
    ("-s", "com.lab126.powerd", "state"): ("power", "active"),
    ("-s", "com.lab126.winmgr", "epdcMode"): ("mode", "Y8"),
    ("com.lab126.winmgr", "accelerometer"): ("holding", "U"),
}
EVENT_ARGUMENTS = ["-m", "-s", "0", "com.lab126.powerd",
                   "goingToScreenSaver,outOfScreenSaver,exitingScreenSaver,wakeupFromSuspend,usbConfigured"]
LIBC = ctypes.CDLL(None, use_errno=True)
LIBC.tee.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_size_t, ctypes.c_uint]
LIBC.tee.restype = ctypes.c_ssize_t
PROFILES = {
    "scribe": ("U:0", ["power", "mode", "holding", "holding", "power"]),
    "pw6": ("FIXED:0", ["power", "mode", "power"]),
}


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def clock():
    return time.clock_gettime(time.CLOCK_BOOTTIME)


def until(predicate, timeout, description):
    deadline = clock() + timeout
    while clock() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(0.005)
    raise AssertionError(f"Timed out waiting for {description}")


def identity(info):
    return (info.st_dev, info.st_ino)


def inode(path):
    return identity(path.stat())


@dataclass(frozen=True)
class Process:
    pid: int
    parent: int
    group: int
    session: int
    birth: int
    state: str
    status: int

    @property
    def key(self):
        return (self.pid, self.birth)

    @property
    def live(self):
        return self.state not in EXITED

    @property
    def root(self):
        return Path("/proc") / str(self.pid)


def process(pid):
    try:
        raw = (Path("/proc") / str(pid) / "stat").read_bytes()
    except (FileNotFoundError, ProcessLookupError):
        return None
    require(raw.startswith(f"{pid} (".encode()), "Unexpected process stat identity")
    fields = raw.rsplit(b") ", 1)[1].split()
    return Process(pid, int(fields[1]), int(fields[2]), int(fields[3]), int(fields[19]),
                   fields[0].decode("ascii"), int(fields[49]))


def same_process(original):
    current = process(original.pid)
    return current if current and current.key == original.key else None


def process_table():
    return {p.pid: p for leaf in Path("/proc").iterdir()
            if leaf.name.isdecimal() and (p := process(int(leaf.name))) is not None}


def descendants(master, table):
    selected = {master}
    while True:
        added = {p.pid for p in table.values() if p.parent in selected} - selected
        if not added:
            return {pid: table[pid] for pid in selected - {master}}
        selected.update(added)


def arguments(member):
    return (member.root / "cmdline").read_bytes().rstrip(b"\0").split(b"\0")


def descriptor_links(member):
    entries = {}
    for leaf in (member.root / "fd").iterdir():
        try:
            entries[int(leaf.name)] = os.readlink(leaf)
        except FileNotFoundError:
            continue
    require(same_process(member) is not None, "Process birth changed during descriptor observation")
    return entries


def descriptor(member, fd, access):
    path = member.root / "fd" / str(fd)
    info = path.stat()
    require(stat.S_ISFIFO(info.st_mode), "Startup descriptor is not a pipe")
    lines = (member.root / "fdinfo" / str(fd)).read_text(encoding="ascii").splitlines()
    flags = int(next(line.split()[1] for line in lines if line.startswith("flags:")), 8)
    require(flags & os.O_ACCMODE == access, "Startup descriptor has an unwanted peer direction")
    return (identity(info), os.readlink(path), flags)


@contextmanager
def pin(member):
    fd = os.pidfd_open(member.pid)
    try:
        require(same_process(member) is not None, "The signal target's birth changed")
        yield fd
    finally:
        os.close(fd)


def send(member, sig):
    with pin(member) as fd:
        signal.pidfd_send_signal(fd, sig)


def stop(member):
    send(member, signal.SIGSTOP)
    until(lambda: (p := same_process(member)) and p.state == "T", 0.5, "exact process to stop")


def continue_if_live(member):
    if (p := same_process(member)) and p.live:
        try:
            send(p, signal.SIGCONT)
        except ProcessLookupError:
            pass


def wait_for_exit(fd, deadline, message):
    poll = select.poll()
    poll.register(fd, select.POLLIN)
    remaining = deadline - clock()
    require(remaining > 0 and any(mask & select.POLLIN for _, mask in poll.poll(max(1, int(remaining * 1000)))),
            message)


def peek_pipe(source):
    """Duplicate queued evidence without consuming it or keeping a writer."""
    require(stat.S_ISFIFO(os.fstat(source).st_mode), "Expected a live fixture pipe")
    reader, writer = os.pipe2(os.O_NONBLOCK | os.O_CLOEXEC)
    try:
        count = LIBC.tee(source, writer, 640, 2)  # SPLICE_F_NONBLOCK
        if count < 0:
            error = ctypes.get_errno()
            if error in (errno.EAGAIN, errno.EINTR):
                return b""
            raise OSError(error, os.strerror(error))
        return os.read(reader, count) if count else b""
    finally:
        os.close(reader)
        os.close(writer)


def peek_fifo(path):
    source = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        return peek_pipe(source)
    finally:
        os.close(source)


def file_record(path, maximum=512):
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        info = os.fstat(fd)
        require(stat.S_ISREG(info.st_mode) and info.st_size <= maximum, f"Unexpected record at {path.name}")
        return os.read(fd, maximum + 1)
    finally:
        os.close(fd)


def absent(path):
    return not os.path.lexists(path)


def mark(path, data):
    pending = path.with_suffix(".pending")
    with pending.open("xb") as stream:
        stream.write(data)
    pending.replace(path)


def getter():
    case = Path(os.environ["BILI_STARTUP_FIXTURE_CASE"])
    me = process(os.getpid())
    if sys.argv[1:] == ["-s", "com.lab126.blanket", "load"]:
        # The optional blanket read remains in the launcher's original scope.
        grant = file_record(LOCK / "native-read-go")
        leader = process(me.group)
        require(leader is not None, "Legacy getter has no leader")
        master = process(leader.parent)
        require(master is not None and leader.pid == me.group == me.session == leader.group == leader.session,
                "Blanket getter escaped its legacy group")
        lead, sequence, token = grant.decode("ascii").rstrip("\n").split(":")
        require(grant == f"{leader.pid}:{int(sequence)}:{token}\n".encode() and int(lead) == leader.pid
                and file_record(LOCK / "pid") == f"{master.pid}\n".encode()
                and file_record(LOCK / "nonce") == f"{token}\n".encode(),
                "Blanket getter lacks the original exact launch grant")
        name, output = "blanket", "screensaver"
    else:
        name, output = DISPLAY_QUERIES[tuple(sys.argv[1:])]
    row = {"name": name, "process": me.__dict__, "entered": clock(),
           "pipes": list(descriptor_links(me).values())}
    with (case / "getters").open("ab", buffering=0) as stream:
        stream.write(json.dumps(row).encode() + b"\n")
    first_power = case / "first-power"
    if name == "power" and absent(first_power):
        if case.name.endswith("-escaped-member"):
            # Exercise subtree retirement separately from the original group.
            # This stays a descendant and cannot keep any stdout or gate end.
            detached = subprocess.Popen(["/bin/sleep", "6"], start_new_session=True,
                                        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                        stderr=subprocess.DEVNULL)
            member = process(detached.pid)
            require(member and member.parent == me.pid and member.group == member.session == member.pid,
                    "Unrelated-session fixture child was not rooted in the getter")
            mark(case / "escaped", json.dumps(member.__dict__).encode())
        mark(first_power, json.dumps(row).encode())
        until((case / "release-getter").exists, 5, "fixture getter gate")
        (case / "getter-returning").touch(exist_ok=False)
    print(output)


def power_source():
    require(sys.argv[1:] == EVENT_ARGUMENTS, "Unexpected native source subscription")
    case = Path(os.environ["BILI_STARTUP_FIXTURE_CASE"])
    deadline = clock() + 14
    event = case / "source-event"
    while clock() < deadline:
        if event.is_file():
            data = file_record(event)
            require(data in (b"goingToScreenSaver 2\n", b"usbConfigured 1\n"), "Unexpected fixture event")
            require(os.write(1, data) == len(data), "Short native source write")
            (case / "source-sent").touch(exist_ok=False)
            break
        time.sleep(0.005)
    # Keep the hardware source open through barrier reconciliation. The driver
    # checks observer cleanup; this is just the hard cap on its fixture peer.
    while clock() < deadline:
        time.sleep(0.02)


def mount(stack, *options, target):
    subprocess.run(["/usr/bin/mount", *options, str(target)], check=True, timeout=5)
    stack.callback(subprocess.run, ["/usr/bin/umount", str(target)], check=True, timeout=5)


def tmpfs(stack, target, options):
    mount(stack, "-t", "tmpfs", "-o", options, "tmpfs", target=Path(target))


def mount_info(path, key):
    visible = subprocess.check_output(["/usr/bin/findmnt", "--uniq", "-n", "-o", key, "-T", str(path)],
                                      timeout=3, text=True).splitlines()
    require(len(visible) == 1, f"Expected one visible mount for {path}")
    return visible[0].strip()


def prepare(stack):
    require(sys.platform == "linux" and Path.cwd() == WORK and os.geteuid() == 0 and WORK.stat().st_uid == 0
            and stat.S_IMODE(WORK.stat().st_mode) == 0o700
            and Path("/proc/1/comm").read_text().strip() == "ubuntu-run"
            and [name for _, name in socket.if_nameindex()] == ["lo"],
            "Requires a dedicated root-owned, loopback-only ubuntu-run jail")
    for target in (Path("/tmp"), Path("/mnt"), Path("/usr/sbin"), Path("/etc"),
                   Path("/dev"), Path("/run"), INVENTORY):
        require(mount_info(target, "PROPAGATION") == "private", f"Non-private fixture target: {target}")
    require(all(mount_info(Path(path), "FSTYPE") == "tmpfs" for path in ("/dev", "/run"))
            and mount_info(Path("/proc/self"), "FSTYPE") == "proc" and INVENTORY.is_file()
            and absent(FRAMEBUFFER) and absent(Path("/dev/input")) and absent(Path("/mnt/us"))
            and absent(Path("/etc/prettyversion.txt")), "Unexpected hardware or mount setup")
    require(all(absent(Path(root) / tool) for root in ("/usr/bin", "/bin", "/usr/sbin", "/sbin")
                for tool in ("lipc-get-prop", "lipc-set-prop", "lipc-wait-event")), "Real LIPC tools are present")
    with ELF.open("rb") as stream:
        header = stream.read(20)
        require(stat.S_ISREG(os.fstat(stream.fileno()).st_mode) and os.access(ELF, os.X_OK)
                and header[:7] == b"\x7fELF\x02\x01\x01" and struct.unpack_from("<H", header, 18)[0] == 62,
                "Expected an executable full Linux x86-64 product ELF")
    require(LIBC.prctl(36, 1, 0, 0, 0) == 0, "Cannot become a private child subreaper")

    tmpfs(stack, "/tmp", "size=24m,mode=1777,nosuid,nodev")
    tmpfs(stack, "/mnt", "size=16m,mode=0700,nosuid,nodev")
    tmpfs(stack, "/usr/sbin", "size=1m,mode=0755,nosuid,nodev")
    work = Path("/tmp/native-startup-product")
    tools = work / "bin"
    tools.mkdir(mode=0o700, parents=True)
    # Reuse the two read-only ioctls, with the current native PW6 dimensions
    # and a harmless character node to pass the complete launcher's -c check.
    fb = (FIXTURE / "fb_read.c").read_text(encoding="ascii")
    for old, new in (("S_ISREG(expected.st_mode)", "S_ISCHR(expected.st_mode)"),
                     ("1264;", "1272;"), ("1680;", "1696;")):
        require(fb.count(old) == 1, "Framebuffer fixture source changed")
        fb = fb.replace(old, new)
    source = tools / "fb_read.c"
    source.write_text(fb, encoding="ascii")
    subprocess.run(["/usr/bin/cc", "-O2", "-Wall", "-Wextra", "-Werror", "-shared", "-fPIC",
                    "-o", str(tools / "fb-read.so"), str(source)], check=True, timeout=15)
    # The older capture fixture only needed two usleep values. The full shell
    # also observes native settling and cleanup with other genuine delays.
    usleep = tools / "usleep"
    usleep.write_text("#!/bin/sh\nset -eu\ncase \"$1\" in *[!0-9]*|'') exit 2;; esac\n"
                      "exec /bin/sleep \"$(($1 / 1000000)).$(printf '%06d' \"$(($1 % 1000000))\")\"\n",
                      encoding="ascii")
    usleep.chmod(0o700)
    for role in (Path("/usr/sbin/lipc-get-prop"), tools / "lipc-wait-event"):
        role.write_bytes(Path(__file__).read_bytes())
        role.chmod(0o700)
    # Never run live service commands even if Ubuntu supplies an identically
    # named tool. Reaching take_display at all is a failure in this fixture.
    for name in ("status", "stop", "start", "pidof", "xrefresh"):
        stub = tools / name
        stub.write_bytes(b'#!/bin/sh\nprintf "%s\\n" "${0##*/}" >>"$BILI_STARTUP_FIXTURE_CASE/takeover"\nexit 1\n')
        stub.chmod(0o700)

    # Keep the actual Ubuntu awk: hiding /etc also hides its alternatives link.
    awk = Path(shutil.which("awk", path="/usr/bin:/bin") or "/missing-awk").resolve(strict=True)
    require(awk.is_file() and awk.parent == Path("/usr/bin") and os.access(awk, os.X_OK),
            "Expected the jail's installed awk")
    (tools / "awk").symlink_to(awk)
    tmpfs(stack, "/etc", "size=1m,mode=0755,nosuid,nodev")
    Path("/etc/prettyversion.txt").write_text(f"Kindle {FIRMWARE} (host fixture)\n", encoding="ascii")
    inventory = work / "inventory"
    inventory.touch(exist_ok=False)
    mount(stack, "--bind", str(inventory), target=INVENTORY)
    zero = Path("/dev/zero")
    dev = zero.stat()
    require(stat.S_ISCHR(dev.st_mode) and (os.major(dev.st_rdev), os.minor(dev.st_rdev)) == (1, 5),
            "Expected the harmless jail /dev/zero device")
    FRAMEBUFFER.touch(exist_ok=False)
    stack.callback(FRAMEBUFFER.unlink)
    mount(stack, "--bind", str(zero), target=FRAMEBUFFER)
    require(inode(FRAMEBUFFER) == identity(dev) and inode(INVENTORY) == inode(inventory),
            "Synthetic hardware mounts are not active")

    binary = Path("/mnt/us/bilicomics/app/bilicomics")
    binary.parent.mkdir(mode=0o700, parents=True)
    binary.touch(exist_ok=False)
    mount(stack, "--bind", str(ELF), target=binary)
    require(inode(binary) == inode(ELF), "Launcher executable is not the staged product")
    return work, binary


class Trial:
    def __init__(self, case, binary, profile):
        self.case = case
        self.binary = binary
        self.profile = profile
        self.home = binary.parent.parent
        self.data = self.home / "data"
        self.audit = case / "fb-audit"
        self.log = self.data / "logs/launcher.log"
        self.trace = self.data / "logs/launcher-timing.tsv"
        self.tree = {}

    def start(self):
        require(absent(LOCK), "A preceding fixture launch did not release the lock")
        self.case.mkdir(mode=0o700)
        self.data.mkdir(mode=0o700)
        (self.data / "device-profile").write_text(self.profile + "\n", encoding="ascii")
        (self.data / "device-allowlist").write_text(f"{self.profile} {FIRMWARE}\n", encoding="ascii")
        inventory = self.case.parent / "inventory"
        inventory.write_bytes(b"B: ABS=1000000\n" if self.profile == "scribe" else b"B: ABS=1000003\n")
        require(inode(inventory) == inode(INVENTORY), "Input inventory no longer points at the fixture")
        self.audit.touch(exist_ok=False)
        tools = self.case.parent / "bin"
        env = {
            "PATH": f"{tools}:/usr/sbin:/usr/bin:/sbin:/bin", "LC_ALL": "C", "HOME": str(self.home),
            "BILICOMICS_HOME": str(self.home), "BILICOMICS_LAUNCH_TRACE": "1",
            "DISPLAY_FIXTURE_FB_AUDIT": str(self.audit), "BILI_STARTUP_FIXTURE_CASE": str(self.case),
            "LD_PRELOAD": str(tools / "fb-read.so"),
        }
        with (self.case / "entry-output").open("xb") as output:
            self.child = subprocess.Popen(["/bin/sh", str(LAUNCH), "--detached"], cwd=self.case,
                                          env=env, stdin=subprocess.DEVNULL, stdout=output,
                                          stderr=subprocess.STDOUT, start_new_session=True)
        self.master = process(self.child.pid)
        require(self.master and self.master.parent == os.getpid()
                and self.master.group == self.master.session == self.master.pid,
                "Not the direct detached launcher")

    def getters(self):
        return [json.loads(line) for line in (self.case / "getters").read_bytes().splitlines()]

    def trace_names(self):
        if absent(self.trace):
            return []
        return [line.split("\t")[1] for line in self.trace.read_text(encoding="ascii").splitlines()
                if line.count("\t") >= 2]

    def verify_live(self):
        first_path = self.case / "first-power"
        until(lambda: self.child.poll() is not None or first_path.is_file(), 7, "first gated startup getter")
        require(first_path.is_file() and self.child.poll() is None, "Launcher did not reach its first display getter")
        first = json.loads(first_path.read_bytes())
        self.getter = Process(**first["process"])
        self.sampler = process(self.getter.parent)
        self.leader = process(self.getter.group)
        require(self.leader and self.sampler and self.getter.session == self.leader.session == self.leader.pid
                and self.leader.group == self.leader.pid and self.sampler.parent == self.leader.pid
                and self.sampler.group == self.sampler.session == self.leader.pid
                and self.sampler.pid != self.leader.pid, "Getter and sampler do not descend from the private leader")
        self.owner = process(self.leader.parent)
        require(self.owner and self.owner.parent == self.master.pid
                and (self.owner.group, self.owner.session) == (self.master.group, self.master.session)
                and self.owner.session != self.leader.session
                and self.master.birth <= self.owner.birth <= self.leader.birth <= self.sampler.birth <= self.getter.birth,
                "Unexpected M/O/L/sampler/getter ancestry")
        executable = inode(self.binary)
        for member in (self.owner, self.leader, self.sampler):
            require(member.live and inode(member.root / "exe") == executable, "Not the full product executable")
        argv = arguments(self.owner)
        require(len(argv) == 8 and argv[1:6] == [b"--native-startup-snapshot", b"owner", self.profile.encode(),
                                                 str(self.master.pid).encode(), str(self.master.birth).encode()]
                and all(re.fullmatch(rb"[1-9][0-9]*", value) for value in argv[6:]),
                "Unexpected native owner invocation or noncanonical visit clock")
        self.scope = argv[4:]
        self.begin = int(argv[-1]) / 100
        require(0 <= first["entered"] - self.begin < 2, "First getter lies outside its original startup read interval")
        self.binding = {leaf: file_record(LOCK / leaf) for leaf in ("pid", "generation", "nonce")}
        self.lock_identity = inode(LOCK)
        self.header = self.binding["generation"].removesuffix(b"\n")
        require(re.fullmatch(rb"[1-9][0-9]*:[0-9a-f]{32}", self.header)
                and self.binding["generation"] == self.header + b"\n"
                and self.binding["pid"] == f"{self.master.pid}\n".encode()
                and self.binding["nonce"] == self.header.split(b":")[1] + b"\n", "Invalid launcher binding")
        require(all(absent(LOCK / leaf) for leaf in ("native-read-go", "native-read-go.pending", "application-pid",
                                                    "lifecycle-request", "native-epdc-mode", "native-portrait-orientation")),
                "Startup reader overlapped legacy read permission, publication or App startup")

        control = descriptor(self.owner, 64, os.O_RDONLY)
        result = descriptor(self.owner, 65, os.O_WRONLY)
        self.result = result[0]
        require(control[0] != result[0] and all(value[1].startswith("pipe:[") and value[2] & os.O_CLOEXEC
                                                 for value in (control, result)), "Owner pipes are not distinct CLOEXEC ends")
        require(descriptor(self.master, 7, os.O_WRONLY)[:2] == control[:2]
                and descriptor(self.master, 8, os.O_RDONLY)[:2] == result[:2]
                and inode(self.master.root / "fd/9") == inode(self.owner.root),
                "Master has not pinned the exact owner and its two role-specific ends")
        self.pipes = {control[1], result[1]}
        for member, expected in ((self.owner, {64, 65}), (self.master, {7, 8})):
            require({fd for fd, target in descriptor_links(member).items() if target in self.pipes} == expected,
                    "Owner or master holds an extra startup pipe end")
        self.tree = descendants(self.owner.pid, process_table())
        chain = {self.leader.pid, self.sampler.pid, self.getter.pid}
        watchdogs = [p for p in self.tree.values() if p.live and p.pid not in chain and p.parent == self.leader.pid]
        require(chain <= self.tree.keys() and len(watchdogs) == 1, "No exact in-group watchdog during the gated read")
        watchdog = watchdogs[0]
        require(watchdog.group == watchdog.session == self.leader.pid
                and self.leader.birth <= watchdog.birth <= self.sampler.birth
                and inode(watchdog.root / "exe") == executable, "Unexpected watchdog birth or topology")
        require(same_process(self.getter) is not None, "The gated getter changed birth before inspection")
        for member in self.tree.values():
            if member.live:
                require(not self.pipes.intersection(descriptor_links(member).values()),
                        "A native descendant inherited a control/result pipe inode")
        require(not self.pipes.intersection(first["pipes"]), "First getter inherited a control/result pipe inode")

    def assert_retired(self, *, strict):
        def no_survivors():
            table = process_table()
            for member in (self.owner, *self.tree.values()):
                current = table.get(member.pid)
                if current and current.key == member.key and (strict or current.live):
                    return False
            return not any((p.group == self.leader.pid or p.session == self.leader.pid) and (strict or p.live)
                           for p in table.values())

        until(no_survivors, 2, "native scope to disappear" if strict else "native scope to stop")

    def assert_no_app(self):
        require(absent(LOCK / "application-pid") and absent(self.case / "takeover")
                and "native_take_begin" not in self.trace_names(), "Launcher attempted display takeover or App startup")

    def assert_read(self, expected_display):
        rows = self.getters()
        require([r["name"] for r in rows] == ["blanket", *expected_display],
                "Wrong native getter sequence or reused read scope")
        require(not any(self.pipes.intersection(r["pipes"]) for r in rows), "A getter retained a startup pipe inode")
        display = [Process(**r["process"]) for r in rows[1:]]
        require(all(g.parent == self.sampler.pid and g.session == g.group == self.leader.pid
                    and self.sampler.birth <= g.birth for g in display), "A query escaped the witnessed sampler")
        return display

    def park_master_across_success(self, grant_fd):
        stop(self.master)
        # Open this tap while O is alive: Linux may suppress HUP on a fresh
        # nonblocking read end opened with no writer. M stays stopped, and the
        # tap never consumes the original bytes or holds another writer.
        fd = os.open(self.master.root / "fd/8", os.O_RDONLY | os.O_NONBLOCK | os.O_CLOEXEC)
        try:
            require(identity(os.fstat(fd)) == self.result, "Master lost the original result pipe")
            (self.case / "release-getter").touch(exist_ok=False)
            exited = until(lambda: (p := same_process(self.owner)) and not p.live and p,
                           1.6, "successful exact owner exit while the master is stopped")
            require(exited.status == 0 and clock() < self.begin + 2, "Owner did not complete the sample early")
            table = process_table()
            require(not descendants(self.owner.pid, table), "Exited owner has a child")
            require(all(member.pid not in table or table[member.pid].key != member.key for member in self.tree.values()),
                    "An exact native child survived owner exit")
            require(not any(p.group == self.leader.pid or p.session == self.leader.pid for p in table.values()),
                    "The native process group survived owner exit")
            poll = select.poll()
            poll.register(fd, select.POLLIN | select.POLLHUP)
            required = select.POLLIN | select.POLLHUP
            require(any((mask & required) == required for _, mask in poll.poll(0)),
                    "Exited owner left its original terminal unframed or a peer writer alive")
            wire = peek_pipe(fd)
        finally:
            os.close(fd)
        pid, birth, sequence, begin = self.scope
        prefix = b":".join([b"startup-display-v1", self.header, sequence, pid, birth, begin,
                            str(self.owner.pid).encode(), str(self.owner.birth).encode(), b"0"]) + b":"
        require(wire.startswith(prefix) and wire.endswith(b"\n"), "Terminal is not bound to the exact startup visit")
        until_ns, sealed_ns, directory_dev, directory_ino, dev, ino, body = wire[len(prefix):-1].split(b":")
        direction, _ = PROFILES[self.profile]
        require(all(re.fullmatch(rb"(?:0|[1-9][0-9]*)", value)
                    for value in (until_ns, sealed_ns, directory_dev, directory_ino, dev, ino))
                and int(until_ns) == int(begin) * 10_000_000 + 2_000_000_000
                and int(begin) * 10_000_000 <= int(sealed_ns) < int(until_ns)
                and (int(directory_dev), int(directory_ino)) == self.lock_identity
                and (int(dev), int(ino)) == identity(os.fstat(grant_fd))
                and os.pread(grant_fd, 512, 0) == b""
                and body == f"ready:Y8:{direction}\n".encode().hex().encode(),
                "Terminal is not the complete sealed capture with the exact invalidated grant")

    def check_success(self, grant_fd, event, park):
        relay_pid = LOCK / "observer-relay-pid"
        until(relay_pid.is_file, 0.7, "real relay PID publication")
        relay = process(int(file_record(relay_pid)))
        require(relay and relay.live and inode(relay.root / "exe") == inode(self.binary)
                and arguments(relay)[1:] == [b"--relay-power-events"], "Power relay is not the product ELF")
        source_pid = LOCK / "observer-pid"
        until(source_pid.is_file, 0.5, "native event producer identity")
        source = process(int(file_record(source_pid)))
        observer = process(relay.parent)
        source_runner = process(int(file_record(LOCK / "observer-runner-pid")))
        require(source and observer and source_runner and observer.parent == self.master.pid
                and source.parent == source_runner.pid and source_runner.parent == observer.pid
                and relay.group == relay.session == source.group == source.session == observer.pid,
                "Unexpected source and relay group")
        source_fds = descriptor_links(source)
        require(source_fds.get(1) == str(LOCK / "power-source") and str(LOCK / "power-events") not in source_fds.values(),
                "Synthetic hardware source can bypass the real relay")
        stop(relay)
        try:
            mark(self.case / "source-event", event)
            until((self.case / "source-sent").exists, 0.5, "synthetic source write")
            require(peek_fifo(LOCK / "power-source") == event, "Event was not queued at the real relay input")
            if park:
                self.park_master_across_success(grant_fd)
                # The original sampler must already have returned; only the
                # master consumes this terminal later, within the hard budget.
                until(lambda: clock() >= self.begin + 2.15, 2.2, "late but still fresh master schedule")
                require(clock() < self.begin + 2.65, "Within-deadline case slipped past its scheduling margin")
                continue_if_live(self.master)
            else:
                (self.case / "release-getter").touch(exist_ok=False)

            generation, token = self.header.split(b":")
            barrier = b"_bilicomics_start_ready " + generation + b" " + token + b"\n"
            until(lambda: peek_fifo(LOCK / "power-control") == barrier, 3.7, "production startup barrier")
            stop(self.master)
            self.assert_retired(strict=True)
            direction, expected = PROFILES[self.profile]
            require(file_record(LOCK / "native-epdc-mode") == self.header + b":Y8\n"
                    and file_record(LOCK / "native-portrait-orientation") == self.header + b":" + direction.encode() + b"\n",
                    "Shell did not publish the exact native startup snapshot")
            require(inode(GRANT) == identity(os.fstat(grant_fd)) and os.pread(grant_fd, 512, 0) == b""
                    and absent(LOCK / "native-read-go"), "Grant was reused or not retired as empty evidence")
            require(all(same_process(g) is None for g in self.assert_read(expected))
                    and self.audit.read_bytes() == b"VVFVVF", "Incomplete read-only framebuffer/getter transaction")
            self.assert_no_app()
            continue_if_live(relay)
            until(lambda: peek_fifo(LOCK / "power-events") == event + barrier,
                  0.8, "real relay output to order the native event before startup")
        finally:
            continue_if_live(self.master)
            continue_if_live(relay)
        expected_status, marker = (1, "sleep_event") if event.startswith(b"goingToScreenSaver") else (143, "usb_takeover")
        require(self.child.wait(timeout=6) == expected_status, "Unexpected shell disposition of pre-takeover power event")
        traces = self.trace_names()
        require(traces.count("startup_sample_ready") == 1 and traces.count(marker) == 1
                and traces.index("startup_sample_ready") < traces.index(marker) and absent(LOCK),
                "Shell did not consume the barrier and native event in order or release its lock")
        self.assert_no_app()

    def check_refusal(self, grant_fd, original, mode):
        old_inode = identity(os.fstat(grant_fd))
        replacement_fd = None
        try:
            if mode == "term":
                with pin(self.owner) as fd:
                    send(self.master, signal.SIGTERM)
                    wait_for_exit(fd, self.begin + 1.8, "SIGTERM did not terminate the owner before capture timeout")
            elif mode == "term-after-seal":
                self.park_master_across_success(grant_fd)
                send(self.master, signal.SIGTERM)
                continue_if_live(self.master)
            elif mode == "owner-death":
                send(self.owner, signal.SIGKILL)
            elif mode == "same-bytes-grant":
                replacement = self.case / "other-grant"
                replacement.write_bytes(original)
                replacement_fd = os.open(replacement, os.O_RDONLY | os.O_CLOEXEC)
                with pin(self.owner) as fd:
                    replacement.replace(GRANT)
                    require(inode(GRANT) != old_inode and file_record(GRANT) == original,
                            "Exact-grant control failed to replace the inode while preserving bytes")
                    wait_for_exit(fd, self.begin + 1.8, "Replacement was not refused before capture timeout")
            elif mode == "late-after-deadline":
                self.park_master_across_success(grant_fd)
                until(lambda: clock() >= self.begin + 3.08, 3.2, "owner result to lose absolute freshness")
                continue_if_live(self.master)
            elif mode == "escaped-member":
                outsider = Process(**json.loads((self.case / "escaped").read_bytes()))
                require(outsider.parent == self.getter.pid and outsider.group == outsider.session == outsider.pid,
                        "Missing a real descendant outside the original group")
                (self.case / "release-getter").touch(exist_ok=False)
            else:
                require(mode == "getter-timeout", "Unknown refusal case")
                with pin(self.getter) as fd:
                    wait_for_exit(fd, self.begin + 2.65, "Getter survived its bounded read timeout")
                    require(clock() >= self.begin + 1.8, "Getter refused before the timeout could matter")
            code = self.child.wait(timeout=7)
            require(code == (143 if mode.startswith("term") else 1), "Refusal did not return the expected launcher status")
            self.assert_no_app()
            require("startup_sample_ready" not in self.trace_names()
                    and all(absent(LOCK / leaf) for leaf in ("native-epdc-mode", "native-portrait-orientation")),
                    "A refused or stale native terminal was accepted")
            if mode in ("late-after-deadline", "escaped-member", "term-after-seal"):
                require(self.audit.read_bytes() == b"VVFVVF", "Refusal did not first complete the real capture")
                self.assert_read(PROFILES[self.profile][1])
            else:
                require(absent(self.case / "getter-returning") and self.audit.read_bytes() == b"",
                        "A cancelled or unauthorized getter advanced to framebuffer I/O")
                self.assert_read(["power"])
            if mode in ("owner-death", "same-bytes-grant", "late-after-deadline", "escaped-member"):
                expected_inode = identity(os.fstat(replacement_fd)) if replacement_fd is not None else old_inode
                require(inode(LOCK) == self.lock_identity and inode(GRANT) == expected_inode
                        and all(file_record(LOCK / leaf) == value for leaf, value in self.binding.items()),
                        "Unproven owner closure discarded the protected launch slot")
            else:
                require(absent(LOCK), "Proven cancellation or timeout refusal did not release the lock")
            if mode != "owner-death":
                require(os.pread(grant_fd, 512, 0) == b"", "Owner did not invalidate the exact grant inode")
            if mode == "owner-death":
                require(os.pread(grant_fd, 512, 0) == original, "Hard-death slot no longer has its grant evidence")
            if replacement_fd is not None:
                require(os.pread(replacement_fd, 512, 0) == original, "Owner modified the unowned replacement grant")
            if mode == "escaped-member":
                table = process_table()
                require(not any(p.group == self.leader.pid for p in table.values())
                        and (p := same_process(outsider)) and p.live and "native_read_cleanup_pending" in self.trace_names(),
                        "An empty original group concealed a live adopted descendant")
                send(p, signal.SIGTERM)
            else:
                self.assert_retired(strict=mode != "owner-death")
        finally:
            if replacement_fd is not None:
                os.close(replacement_fd)

    def verify(self, mode):
        self.verify_live()
        fd = os.open(GRANT, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
        try:
            grant = os.fstat(fd)
            require(stat.S_ISREG(grant.st_mode) and grant.st_nlink == 1 and grant.st_uid == 0
                    and 0 < grant.st_size <= 512 and grant.st_mode & 0o077 == 0,
                    "Startup permission was not a private regular grant")
            original = os.pread(fd, 512, 0)
            pid, birth, sequence, begin = self.scope
            prefix = b":".join([b"startup-display-v1", self.header, sequence, pid, birth, begin] + [
                str(value).encode() for member in (self.owner, self.leader) for value in member.key]) + b":"
            require(original.startswith(prefix) and re.fullmatch(rb"[1-9][0-9]*\n", original[len(prefix):]),
                    "Grant does not identify the witnessed startup owner, leader and visit")
            if mode in ("sleep", "usb", "late-before-deadline"):
                event = b"goingToScreenSaver 2\n" if mode == "sleep" else b"usbConfigured 1\n"
                self.check_success(fd, event, park=mode == "late-before-deadline")
            else:
                self.check_refusal(fd, original, mode)
        finally:
            os.close(fd)


def reap_adopted():
    for _ in range(128):
        try:
            if os.waitpid(-1, os.WNOHANG)[0] == 0:
                return
        except ChildProcessError:
            return


def clean_trial(trial, failed):
    # This controller is a child subreaper in its own PID namespace. Cleanup
    # is restricted to actual descendants, including reparented orphans, and
    # always targets a pinned birth. Never trust a PID alone from a lock file.
    child = getattr(trial, "child", None)
    if child is not None and child.poll() is None:
        master = getattr(trial, "master", None)
        if master and (p := same_process(master)) and p.live:
            try:
                send(p, signal.SIGTERM)
                continue_if_live(p)
                child.wait(timeout=2)
            except (ProcessLookupError, subprocess.TimeoutExpired):
                pass
    cleanup_error = None
    if not failed:
        try:
            until(lambda: not any(p.live for p in descendants(os.getpid(), process_table()).values()),
                  2, "launcher and observer cleanup")
        except AssertionError as error:
            cleanup_error = error
    deadline = clock() + 2
    while clock() < deadline:
        if child is None or child.poll() is not None:
            reap_adopted()
        remaining = descendants(os.getpid(), process_table())
        if not remaining:
            if cleanup_error:
                raise cleanup_error
            return
        for member in remaining.values():
            if member.live:
                try:
                    send(member, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        time.sleep(0.01)
    raise AssertionError("Private fixture descendants did not retire within the cleanup cap")


def diagnostics(trial):
    print(f"Launcher status: {trial.child.poll()}; trace: {trial.trace_names()}", file=sys.stderr)
    if not absent(LOCK):
        print(f"Retained leaves: {sorted(p.name for p in LOCK.iterdir())}", file=sys.stderr)
    if (trial.case / "getters").is_file():
        print(f"Getter stages: {[row['name'] for row in trial.getters()]}", file=sys.stderr)
    for path in (trial.case / "entry-output", trial.log):
        if path.is_file():
            with path.open("rb") as stream:
                stream.seek(max(0, os.fstat(stream.fileno()).st_size - 2048))
                tail = stream.read(2048).decode("utf-8", errors="replace")
            if tail:
                print(re.sub(r"\b[0-9a-f]{32}\b", "<launch-token>", tail), file=sys.stderr)


def main():
    require(len(sys.argv) == 1, "This driver uses only the fixed private jail paths")
    os.umask(0o077)
    with ExitStack() as stack:
        work, binary = prepare(stack)
        cases = [("scribe", "sleep"), ("pw6", "usb"), ("pw6", "late-before-deadline"),
                 ("scribe", "term"), ("pw6", "owner-death"), ("scribe", "same-bytes-grant"),
                 ("pw6", "getter-timeout"), ("scribe", "late-after-deadline"),
                 ("pw6", "escaped-member"), ("pw6", "term-after-seal")]
        for profile, mode in cases:
            trial = Trial(work / f"{profile}-{mode}", binary, profile)
            failed = True
            try:
                trial.start()
                trial.verify(mode)
                failed = False
            except Exception:
                print(f"Failed: {profile} {mode}", file=sys.stderr)
                diagnostics(trial)
                raise
            finally:
                clean_trial(trial, failed)
            # Retained evidence is checked before removing it for the next
            # case, and only after cleanup inside this disposable mount.
            if not absent(LOCK):
                shutil.rmtree(LOCK)
            shutil.rmtree(trial.data)
            print(f"Passed: {profile} {mode}", flush=True)
    print("Full host ELF and unchanged candidate shell; no UI, firmware, panel, or timing qualification.")


if __name__ == "__main__":
    role = Path(sys.argv[0]).name
    if role == "lipc-get-prop":
        getter()
    elif role == "lipc-wait-event":
        power_source()
    else:
        main()
