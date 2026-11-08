# BiliComics Kindle: Technical Handoff

Recorded: 2026-11-03 (Asia/Shanghai).

Historical status: source review before the design was supplied. The user subsequently supplied the design and authorized implementation. See [implementation status](IMPLEMENTATION_STATUS.md) and [design provenance](DESIGN_SOURCE.md) for current work. The original preparation notes below retain their original scope and evidence boundaries.

## Request and current state

The user wants a Kindle application for Bilibili Comics, referring to the existing local WeRead application for implementation context and `moooyo/bilicomics.koplugin` for upstream interface usage. The immediate deliverable is this handoff.

The destination is `D:\Code\bilicomics`, with Git remote `git@github.com:moooyo/bilicomics.git`. At the start of this task it contained only Git metadata and had no commits or application implementation. This task adds documentation, without copying an application scaffold or importing the WeRead design.

The user's forthcoming design is the input for product and UI work. Screen inventories, reading gestures, supported device layouts, paid features, download scope, and visual values remain unspecified for this application.

## Reference provenance

| Reference | Inspected source | Evidence boundary |
| --- | --- | --- |
| WeRead application | `C:\Users\moooyo\Documents\Codex\2026-11-02\d\outputs\weread`, local working tree read on 2026-11-03 | HEAD was `71b8456f39a51af636764ffd45b1c2c4c0efb8e8`, but substantial implementation files were untracked and existing documents were modified. That commit does not reproduce the inspected application. Record a separate file snapshot or content digests when code is actually reused. |
| Bilibili protocol reference | [`moooyo/bilicomics.koplugin`, revision `a05ffb987c2089ca389cb3462aafd3ee1c0fa36c`](https://github.com/moooyo/bilicomics.koplugin/tree/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c) | Source-derived contracts and upstream project reports; no live service or account behavior was verified by this project. |
| Application design | To be supplied by the user | No WeRead or KOReader screen, gesture, token set, or feature list has been adopted as the BiliComics design. |

The protocol checkout was read outside this repository. Research checkouts, reference binaries, credentials, downloaded comics, and existing design assets are not handoff deliverables.

## Engineering context from WeRead

The inspected WeRead application is an independent Rust and Slint executable with its own UI, data directory, launcher, and Kindle platform boundary. It uses Tokio, reqwest with rustls, SQLite, and local files. It does not require KOReader or Jobman to run. This provides a concrete architecture to evaluate when implementing the new application; it does not select dependency versions for BiliComics.

Paths in the following tables are relative to the WeRead reference root above.

| Candidate infrastructure | Source entry points | Relevance and limits |
| --- | --- | --- |
| Kindle platform boundary | `src/platform/mod.rs`, `profile.rs`, `native.rs`, `refresh.rs` | Display, input, geometry, hardware capabilities, refresh queue, and lifecycle are separated from business logic. Device profiles and existing results do not establish this application's support matrix. |
| Reviewed display backend | `vendor/slint-backend-kindle/UPSTREAM_REVISION`, `PATCHES.md` | The reference retains an upstream revision and local patches. Preserve this provenance and license material if reused. |
| Chinese input | `src/input.rs`, `assets/pinyin/`, `ui/auth.slint` | Offline pinyin model and touch keyboard are available as implementation references. Authentication-specific behavior is WeRead-specific. |
| Local cache | `src/cache.rs` | Digest-based filenames, LRU, pinned objects, temporary files, and recovery patterns are relevant. Existing cache budgets and image transformations are not comic defaults. |
| Background work | `src/jobs.rs` | Task scheduling, cancellation, and `account_epoch` isolation provide useful patterns for rejecting stale results after an account switch. |
| Persistence | `src/store.rs`, `src/session.rs` | SQLite migrations, transactions, and separately persisted account state are useful patterns. The text-book schema and credential structure do not match Bilibili. |
| UI primitives | `ui/widgets.slint` | Existing buttons, headers, and pagination components can be assessed after the design arrives. Their composition and numeric values are not accepted design decisions here. |
| Remote development | `scripts/verify-remote.ps1`, `scripts/build-kindle.sh` | Windows-to-`ssh test-env` workflow and ARM cross-compilation references. These scripts currently target WeRead paths and artifacts. |
| Kindle packaging | `packaging/kindle/README.md`, `launch.sh` | Application/data separation, model/firmware allowlisting, cleanup, and stock-UI restoration are relevant. Package identity, paths, and environment variables belong to WeRead. |

The reference development combination is Rust 1.92, Slint 1.17.1, and its reviewed backend revision. These are source observations, not a BiliComics toolchain decision or a successful build of this project.

## Business code that does not transfer directly

| Reference area | Reason for separate implementation |
| --- | --- |
| WeRead `src/source/` | WeChat QR, OTP, API keys, signatures, text delivery, and reading-time reports are unrelated to Bilibili's protocol. |
| WeRead `src/domain.rs` and database schema | Books, text chapters, and character offsets do not represent comic episodes, ordered source images, or image-relative positions. |
| WeRead `src/reader.rs`, `src/article.rs`, `ui/reader.slint` | Text pagination, article layout, fonts, and highlights are not a comic image reader. The design has not yet selected comic reading behavior. |
| WeRead `src/book_assets.rs` | Its image preparation is tied to text resources and includes a fixed Scribe-sized output. It is not a general reader for Paperwhite pages or very tall source images. |
| Plugin `bilicomics/reader/` | The plugin delegates rendering and navigation to KOReader ReaderUI and a MuPDF-backed Document provider. That runtime integration is not a standalone Slint reader. Its immutable source descriptors and source-anchor handling are useful concepts to inspect. |
| Plugin Lua controllers and widgets | Protocol semantics can be studied separately from KOReader-specific UI, worker, bootstrap, and lifecycle code. |

## Bilibili protocol entry points

[Protocol source notes](API_REFERENCE.md) contain the detailed source references. The principal investigation paths are:

| Area | Source observations |
| --- | --- |
| Authentication | Bilibili QR/session management, cookie domain handling, account checks, and manga site-context initialization are separate concerns. Successful passport login alone is not the complete manga-session setup. |
| Catalog | The reference contains bookshelf, reading history, search, comic detail, and bookstore/category calls. These capabilities are evidence, not a selected screen list. |
| Reading acquisition | Episode image-index retrieval, an ordered immutable descriptor, short-lived image tokens, and page download/decryption form a pipeline. A cover URL is not chapter content. |
| Native protocol | Some requests need `ultra_sign` and `x-bili-data-sn`; the reference also uses native signing/index/image processing and request-specific private ECDH context. This is a material porting dependency beyond HTTP request construction. |
| Progress | The observed remote `AddHistory` call identifies a comic and episode. Exact source page and viewport restoration are local state; page-level cloud synchronization was not established. |
| Account writes | The source includes favorites and purchase-related operations. Their presence does not authorize or perform account changes in this task, and synthetic purchase evidence does not prove real charging behavior. |

Important integration details already visible in source include different query strings for the actual request and signing input, cookie/CSRF handling, and image-token state that must remain with the matching client or worker. The API appendix records these distinctions so that a later implementation does not flatten them into a generic REST client.

## Porting risks to carry forward

- The native protocol layer has its own source, binary, ABI, and runtime dependencies. The reference WeRead executable targets ARM musl, while the plugin's native compatibility history includes glibc. A supported bridge or port has not been selected for this application.
- Temporary signed image URLs are transport state rather than stable comic/page identity. The plugin separates source descriptors from mutable addresses and checks content before accepting recovered downloads.
- Long images and decoded page buffers have different memory costs from text and thumbnails. WeRead image limits, fixed output dimensions, font bundles, and observed process memory cannot be treated as BiliComics budgets.
- The inspected Kindle backend exposes a single pointer. Multitouch pinch, EMR pen input, and hardware inversion are not established by its existing implementation or by their appearance in a reference design.
- Existing launchers contain model/firmware acceptance boundaries and restore system state on exit or takeover. Reusing the launcher involves application identity and lifecycle work, not only renaming an executable.
- Reference code, native binaries, Slint, backend patches, and fonts have separate license/provenance considerations. No license or distribution model has been selected for this repository.

## Evidence and pending inputs

Only source and documentation were inspected for this handoff. No tests, builds, application launches, protocol probes, login, purchase, or physical-device checks were performed for BiliComics.

The plugin reports successful real-account QR login and a complete 45-page online/download/offline workflow for identified earlier snapshots. Later source and historical reports are not interchangeable. Its older protocol README contains limitations superseded by subsequent work; consult the pinned source and the later evidence records together. Actual credential rotation, actual charging, and physical Scribe compatibility remain separate evidence limits. These upstream reports do not become acceptance results for a new Rust application.

Pending inputs and decisions are the user's design, the intended device/firmware scope, the application behaviors implied by that design, the native-protocol integration approach, and code reuse/license choices. They are intentionally left open here.

Working constraints remain: user-facing discussion in Chinese; code, comments, and repository documentation in English; Windows commands in PowerShell; no local verification without explicit authorization for the current task. When verification is subsequently needed without such authorization, use `ssh test-env`; an unavailable worker does not permit a local fallback.

When the design arrives, use it alongside this handoff and the protocol appendix to identify implementation work and remaining interface gaps. Update source provenance at the point of reuse, rather than treating a mutable local reference or an upstream report as a verified implementation of this project.
