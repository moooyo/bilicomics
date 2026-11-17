# Native-query helper link ABI audit

Date: 2026-11-17, Asia/Shanghai.

The initial BiliComics ARM package checker rejected an unexpected direct
dependency on `libpthread.so.0`. The rejected package and its failed report
are retained unchanged. This audit explains the actual link ABI before the
checker is updated; it is not firmware or physical-device acceptance.

## Observed artifact and symbols

The helper built with Zig 0.15.2 is ELF32 ARM EABI5 hard float, PIE, with
interpreter `/lib/ld-linux-armhf.so.3`, a non-executable stack, GNU_RELRO,
and immediate binding. Its SHA-256 is
`ecb336e4461dcfd5ac436af72d82661d5cc0a1dfe1b46434b00788160dc30268`.
The C input SHA-256 is
`d8e7301a59b5fe691d7bfb9512f7d447da166de75c4acad66a740aee9a5ce3d2`.
No C or protocol change was needed for this audit.

The direct dependency set is exactly `libc.so.6`, `libdl.so.2`, and
`libpthread.so.0`. `readelf --dyn-syms` and the version-need table bind
`read`, `write`, `__errno_location`, and `sigaction` to the pthread namespace,
all at `GLIBC_2.4`. The remaining ordinary C and dynamic-loader imports use
libc and libdl. There is no `pthread_*` function import, helper TLS segment,
or thread creation in the C source. The firmware's liblipc may have its own
dependencies; this audit covers the helper's direct ELF contract.

The verbose compiler output shows the driver's generated ABI stubs ordered
as libm, libpthread, libc, and libdl before the remaining default libraries.
It already enables `--as-needed`. The four ordinary C functions are resolved
from libpthread first, so that library is an actual requirement of this
linked artifact rather than an unused NEEDED entry.

## Isolated link experiments

All experiments ran in a new directory on `test-env`, using copies of the
same C source and build script. They did not modify the frozen application
source, rebuild Rust, execute ARM code, connect to a device, or replace a
previous package.

| Link change | Observed result |
| --- | --- |
| Original build script | Reproduced the exact helper SHA-256 above |
| Add `--as-needed` | Produced the same helper bytes and dependencies |
| Add explicit `-lc` | Produced the same helper bytes and dependencies |
| Add `-nodefaultlibs -lc` | Produced the same helper bytes and dependencies |
| Pass `-Wl,-lc` to prioritize libc | Rejected by the Zig driver as an unsupported linker argument |
| Pass `-l:libc.so.6` | Failed because the generated stub is not available through that generic library search |

No workaround based on generated private cache paths, hand-assembled CRT
inputs, edited dynamic tables, or new low-level syscall wrappers is adopted.
The supported compiler invocation is preserved.

## Checked boundary

Only `bilicomics/app/bilicomics-native-query` may use this dynamic ABI. Its
selected SHA-256 must match the packaged bytes. The checker requires the
exact three dependencies, the fixed interpreter, ARM hard float, PIE,
NX/RELRO/NOW, no alternate loader search paths or text relocations, and
glibc versions no newer than 2.4. It independently maps versioned undefined
function imports to their version-need library and requires exactly the four
reviewed pthread compatibility symbols at `GLIBC_2.4`. Other pthread APIs
are rejected, including a pthread-prefixed name assigned to another library.
The application and JPEG worker continue to require static executables.

This audit explains and constrains the build output. Separate authoritative
package verification and physical ABI tests are still required.
