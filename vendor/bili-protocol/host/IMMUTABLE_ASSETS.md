# Immutable signing input

The original path-based `biliwasm_run` ABI remains available. The application
also uses `biliwasm_run_bytes` with an immutable, SHA-256-pinned Rust snapshot.
It avoids reopening, rereading, and rehashing the fixed module for each signature.
The source cache has one entry per crypto provider. Any newly loaded bytes are
hashed against the existing fixed pin; failures are not negatively cached.

File identity and timestamp metadata are only invalidation hints. The executed
buffer is always the previously verified snapshot. A detected deletion,
replacement, invalid type, or changed stamp invalidates the path entry. Even if
the filesystem cannot report a same-metadata change, the cache cannot execute
unverified replacement bytes: it still owns its old pinned snapshot. An in-flight
call keeps its own Arc until the synchronous C call and cleanup have returned.

The bytes ABI borrows its buffer and never adds it to the host allocation list
or frees it. It rejects null, empty, and oversized inputs before interpretation.
Its length is still reserved against the existing host arena admission budget,
preserving the path loader's accounting. The fixed linear-memory bound remains
unchanged. Neither bound describes total process RSS.

No parsed module, mutable Go host object, runtime, global, timer, private key, or
request result is reused. wasm3 module parsing and compilation bind instance
addresses and retain references to the original bytes; there is no supported
cross-runtime rebinding API in this pinned source. Each invocation therefore
continues to create and release its own environment, runtime, and module under
the existing process serialization lock.

Synthetic source tests check one read and one hash across repeated warm loads,
change/deletion invalidation, old-snapshot independence, and retry after repair.
Native ABI tests use only an empty synthetic module and verify matching failure
cleanup for path and byte inputs. These are test entries awaiting the designated
remote run, not a measured live-service or Kindle speedup.
