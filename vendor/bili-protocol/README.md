# Native protocol sources

This directory contains reviewed source dependencies for the independent Rust
client. Cargo compiles these sources into static libraries for its Linux target,
including `armv7-unknown-linux-musleabihf`. No prebuilt glibc library is selected,
and the build never downloads source or executable assets.

The host runs the pinned official signing WASM through wasm3. It implements the
required Go `syscall/js` host operations and has no browser, HTTP, account or shell
dependency. Calls must be serialized because cJSON has process-global state.
The caller supplies an already verified local WASM path. WASM assets are acquired
separately by the application and are not redistributed here.

The portable library supplies P-256, AES-256 ECB, PBKDF2-HMAC-SHA512 and
authenticated AES-GCM decryption. A small local adaptation adds AES-192 ECB
decryption for the Rust response decoder. The Rust adapter owns CBC chaining,
padding checks, CTR counter widths and image-container handling. All callers
must follow the buffer and return-value contracts in the included headers.

## Source provenance

| Directory | Source | Revision | License |
| --- | --- | --- | --- |
| `host`, `portable`, `legacy_v5` wrappers | [bilicomics.koplugin](https://github.com/moooyo/bilicomics.koplugin) | `a05ffb987c2089ca389cb3462aafd3ee1c0fa36c` | No project-wide license is inferred |
| `wasm3/source` | [wasm3](https://github.com/wasm3/wasm3) | `5fe766c933c7595d728d6172bb1a197607d85b4e` | MIT, see `wasm3/LICENSE` |
| `cjson` | [cJSON](https://github.com/DaveGamble/cJSON) | `c859b25da02955fef659d658b8f324b5cde87be3` | MIT, see `cjson/LICENSE` |
| `legacy_v5/vendor/mbedtls` | [Mbed TLS](https://github.com/Mbed-TLS/mbedtls) | `068ff080b369adfac81509f9b57b2afabaf82dc5` (`3.6.7`) | Apache-2.0 option, see its `LICENSE` |

The Mbed TLS subset is copied from the plugin's patched source snapshot. Its
upstream dependency manifest, patch files and original vendored hash map are
preserved under `legacy_v5`. The original hash map describes those Mbed TLS
files; it is not a hash manifest of the local wrappers or this complete directory.
The Go ABI reference used by the upstream wrapper is
[`wasm_exec.js` at Go 1.25.0](https://github.com/golang/go/blob/go1.25.0/lib/wasm/wasm_exec.js).

## Platform boundary

`src/source/native_build.rs` builds only for Linux. Other platforms must return
an explicit capability error from the Rust protocol adapter. No Windows RNG,
clock or POSIX compatibility implementation is assumed by these sources.

This source integration does not itself establish successful live API access,
actual account authentication, image access or physical Kindle compatibility.
Verification runs on `ssh test-env` unless the current task separately authorizes
another verification host.
