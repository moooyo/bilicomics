# Aidoku native-query implementation provenance

Date: 2026-11-17, Asia/Shanghai.

The user requested that BiliComics adopt the applicable startup improvements
completed in AidokuForKindle. This directory records the separately linked,
read-only native LIPC helper and its parent transport. It is independent of
the previously frozen startup source group in `../aidoku-startup-20261117/`.
It contains no account data, firmware libraries, device captures, timing logs,
or complete research archive.

## Pinned sources

Source repository: [AidokuForKindle](https://github.com/moooyo/aidokuForKindle).
The exact implementation commit is
[`4e069260fa612e465b8fde8730596a0c60e0261f`](https://github.com/moooyo/aidokuForKindle/commit/4e069260fa612e465b8fde8730596a0c60e0261f).
The source package declares `GPL-3.0-or-later`; its complete original GPLv3
text is retained as [LICENSE.AIDOKU](LICENSE.AIDOKU). These source-specific
records do not select an overall BiliComics license or a published release.

| Original source | Retained snapshot | BiliComics adaptation |
| --- | --- | --- |
| `src/native_query.rs` | `upstream/4e06926/src/native_query.rs` | Bounded parent-owned session, fixed read-only opcodes, raw returned text, deadline and cancellation handling, and one transport choice for native display sampling |
| `src/native_query/worker.c` | `upstream/4e06926/src/native_query/worker.c` | Independently linked `bilicomics-native-query` helper, BiliComics test identity, fixed firmware library paths, and no setter or arbitrary property interface |
| `scripts/build-native-query.sh` | `upstream/4e06926/scripts/build-native-query.sh` | Keep the firmware glibc ABI separate from the static musl application and JPEG worker; explicitly request a non-executable stack |

The three original code files and license are unchanged Git-blob exports.
[UPSTREAM_FILES.json](UPSTREAM_FILES.json) binds each retained path to the
full commit, original path, byte length, and SHA-256. The selected archive was
extracted and hashed on the designated `test-env` worker. Package verification
also pins these identities independently rather than accepting a recomputed
manifest as evidence of different upstream sources.

## Integration boundary

The helper opens only the firmware's `/usr/lib/liblipc.so.1` or
`/usr/lib/liblipc.so.1.0`. It exposes eight fixed read-only queries through a
bounded binary protocol. No firmware library or third-party binding library
is redistributed. The helper is compiled for `arm-linux-gnueabihf.2.4`,
Cortex-A8, VFPv3-D16, hard float, PIE, stack protection, RELRO, immediate
binding, and a non-executable stack. The Rust application and JPEG worker
retain their static musl build and package restrictions.

The reviewed Zig 0.15.2 helper has exactly three direct shared dependencies:
`libc.so.6`, `libdl.so.2`, and `libpthread.so.0`, each using only glibc symbol
versions at or below `GLIBC_2.4`. Its driver resolves the ordinary libc APIs
`read`, `write`, `__errno_location`, and `sigaction` through the pthread ABI
stub before libc. The helper does not import a `pthread_*` API or implement
threading. The source-only `LINK_ABI_AUDIT.md` records the inspected symbol
ownership and isolated link experiments. The checker requires this exact
dependency set and those four pthread-bound `GLIBC_2.4` imports; it does not
extend the dynamic exception to the application or JPEG worker.

BiliComics preserves its existing native-display sampling order and total
deadline, native-read ownership checks, framebuffer observations, and
startup/observed-resume callers. A helper unavailable before any query may
select the existing CLI transport once. A connected session error cannot
splice later CLI results into partially sampled helper data. Returned text
continues through BiliComics' strict native-record validation. Helper process
ownership and bounded cleanup remain separate from application lifecycle,
orientation, input barriers, refresh policy, and native-control recovery.

Aidoku's synchronous first-view property reads do not match BiliComics'
background battery and light-control reads. Those paths are not imported as
part of this adaptation, and no new status queries are added for parity.

## Verification and distribution scope

Aidoku timings and hardware reports retain their original scope; they do not
establish BiliComics performance, firmware compatibility, or physical-device
acceptance. BiliComics requires its own source-bound remote checks and matched
physical comparison. New authoritative builds use separate Cargo targets for
each source variant. All builds and verification run through `ssh test-env`;
physical operations additionally require a fresh device lease and complete
native-control restoration.

Development archives include the executable at
`bilicomics/app/bilicomics-native-query` and only this README, the complete
original license, and the upstream manifest in `notices/aidoku-native-query/`.
Selected source snapshots remain in the repository. The package checker binds
the helper to the selected SHA-256, permits its exact firmware interpreter
and reviewed libc/libdl/libpthread dependencies, checks glibc symbol-version
limits, function-import ownership and ELF hardening, and preserves the
App/JPEG worker static-link checks. The first rejected package and its
verification evidence remain separate from later corrected verification.
