# Protocol Implementation Record

Updated on 2026-11-04 (Asia/Shanghai).

The Rust source adapter in `src/source/` implements the Bilibili Comics request shapes reviewed at upstream commit `a05ffb987c2089ca389cb3462aafd3ee1c0fa36c`. The source-reading appendix is [API_REFERENCE.md](API_REFERENCE.md). This record describes implementation facts and evidence; it does not define screens or expand the supplied design.

## Implemented boundary

- Account exchanges include QR generation/polling, official manga `buvid3` initialization, navigation identity validation, domain-scoped cookies, and explicit cookie-renewal/confirmation operations. A confirmed login returns an inactive session candidate. The application persists that candidate before activating it.
- Catalog operations cover favorites, history, search, comic detail, recommendations from the official page context, category metadata, and signed category pages. Episode normalization preserves fractional chapter order and distinguishes permanent, temporary, locked, unavailable, and unknown access.
- Image acquisition separates stable page descriptors from transient source paths and token URLs. Each token batch owns its P-256 context. Downloads exclude account cookies on the CDN, preserve signed URL bytes, recognize ordinary images even when the encryption flag is set, and implement encrypted-container versions 3, 5, 6, 7, and 8.
- Wallet, quote, discount/card, explicit purchase, recharge configuration/order/history, favorite mutation, and chapter-history methods are callable service boundaries. Account-bound purchase/order methods prevent dispatch under a different active account. These methods do not provide application-level quote approval or a durable purchase journal, and no transaction is retried automatically.
- Errors expose bounded safe codes and status values. Debug output omits credentials, acquisition URLs, private keys, QR keys, and reconciliation identifiers. A validated reconciliation identifier can be retained separately after an uncertain order result.

## Native integration

`src/source/native_build.rs` statically compiles the reviewed C WASM host, pinned wasm3/cJSON sources, and the portable Mbed TLS subset for the Cargo Linux target. `vendor/bili-protocol/` contains source provenance, licenses/notices, and the narrow AES-192 wrapper added for protected responses. The code does not link the plugin's prebuilt glibc shared libraries into the Kindle musl executable.

The official signing WASM is a separate, digest-pinned public asset, acquired lazily by the worker-side client. It is not a build-time network dependency. Protected calls sign the exact serialized body that is subsequently sent. The browser fingerprint is not implemented: the adapter retains the reviewed official error-reporting `m2` path. A three-request anonymous public-metadata check on 2026-11-04 successfully used that path for `ClassPage`, including its encrypted response. This does not establish authentication, account entitlement, other protected operations, or future service acceptance.

Native protocol operations currently require Linux. Other operating systems return a typed capability error instead of reporting successful signing or image decryption. Successful Linux compilation does not establish ARM, firmware, or physical-device compatibility.

## Persistence and transaction boundaries

`AuthSession` serialization is intended only for local credential storage. `SessionFile` checks ownership, regular-file status, size, and symlink boundaries and publishes replacement files atomically. POSIX permissions are enforced where supported. The recognized FAT/exFAT case permits only the explicitly handled unsupported-mode errors; FAT storage does not supply POSIX private ACLs or an encrypted credential vault.

The [cookie management review](COOKIE_MANAGEMENT_REVIEW.md) records the subsequent
schema-3 scope and expiry fixes, active-client persistence, response-cookie merge,
and guarded renewal activation. The active application opts into durable session
updates; import and QR candidates remain isolated until explicit activation.
Post-publication storage uncertainty closes authenticated admission rather than
letting older memory overwrite a newly published file.

Renewal and confirmation return candidates for application-controlled durable saving. In-memory blocked markers prevent same-process replay after uncertain results. A persistent attempt journal and crash reconciliation remain application responsibilities; the source adapter alone does not establish a crash-safe renewal transaction.

Those inactive-candidate APIs remain available. The active application now uses
the guarded activate variants: the worker saves and publishes the candidate under
its precise source version before returning to the UI. With active-client storage
attached, blocked attempt markers are also saved before transmission. The
application's durable intent journal and explicit uncertain-result handling remain
necessary; activation does not infer a successful renewal from an unknown response.

Cloud history records only the comic and chapter identifiers. Page identity, source coordinates, reading mode, and zoom belong to the local `ReadingAnchor`. Serialized image descriptors exclude transient paths; an incomplete download reopened after restart obtains a fresh source index rather than treating an old URL as permanent content identity.

Image verification reports `container`: PNG chunk CRCs, JPEG termination, WebP RIFF boundaries, dimensions, and content hashes are checked without decoding all pixels. Reader decoding and raster-memory limits are separate responsibilities.

## Verification evidence

All verification for this implementation runs on `ssh test-env`; no local build, test, or runtime probe was performed. The isolated source harness is `/root/bilicomics-build-20261103/source-check` and shares the application's remote Cargo target directory.

The remote source harness compiled the static native sources and passed 45 offline tests. Coverage includes reviewed synthetic response/image golden vectors, P-256 operations, the QR/site-initialization/navigation state machine, cookie-domain and session persistence behavior, FAT/exFAT permission handling, entitlement normalization, image-container checks, and payment/recharge failure semantics. Production client fixtures also exercise the complete ordered-index/token/CDN chain, plain images carrying the encryption flag, noncredential cookie updates, anonymous category metadata, account-bound transaction dispatch, and cancellation of prepared image batches after an account switch.

A subsequent targeted image run compiled the final source without warnings and passed both image checks after strengthening duplicate-PNG-header rejection. It did not repeat unrelated account or transaction cases.

Those earlier fixtures use synthetic credentials and controlled transports. They performed no real account session, service API probe, purchase, coupon consumption, recharge, or physical Kindle execution. The official signing WASM callback was not executed by those 45 fixtures; the C host was compiled and protected-request construction used an injected fixture signer. Upstream acceptance records remain upstream evidence and do not replace verification of this application.

The subsequent [pre-device verification](PRE_DEVICE_VERIFICATION.md) adds explicit opt-in execution of the digest-pinned official module through `NativeCrypto`, independent Go/JS comparison, and a separate ARM test executable. The query validation now rejects an actual NUL before the C host can truncate it. These tests retain their own source and executable identities. Provisioning a verified public asset in a test runner is separate from testing the production client's lazy download and atomic-publication path, and neither establishes real-account authentication.
