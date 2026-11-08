# Bilibili Comics Source Reading Notes

Recorded on 2026-11-03 from [`moooyo/bilicomics.koplugin` at `a05ffb987c2089ca389cb3462aafd3ee1c0fa36c`][upstream]. This appendix records source observations for the Kindle application handoff. It is not a product specification, an official API contract, or a proposed screen design. Product behavior and layout remain open for the forthcoming design files.

Only public source files and existing upstream evidence were read for this handoff. No application, test, build, account login, service probe, or purchase was executed. Upstream verification claims below belong to that plugin and its recorded source snapshots; they do not verify this new application.

## Request and response observations

The main client uses `https://manga.bilibili.com` and JSON POST requests to `/twirp/{service}/{endpoint}`. Its common wire query is `device=pc&platform=web&nov=27&a=810`. Its signing input is separately fixed to `device=pc&platform=web&nov=27&eot=812`; these strings differ in the source. `ComicDetail`, `ClassPage`, `GetImageIndex`, and `ImageToken` receive an `ultra_sign` query parameter and an `x-bili-data-sn` header. The final serialized JSON body is passed to the signer and then transmitted unchanged. See [request construction][client-post] and [signing adapter][native-sign].

The client sends a browser User-Agent, `Accept`, `Content-Type: application/json;charset=UTF-8`, manga-site `Origin` and `Referer`, and cookies selected for the destination host. On the manga host, it URL-decodes `XSRF-TOKEN` into `x-xsrf-token`. See [headers][client-headers]. This is observed request construction, not evidence that a different client's headers will be accepted.

Responses have a business `code` and usually `data`. A nonzero business code is handled before encrypted response data. A nonempty `bytesData` is decoded with the request context, after which `data` is consumed. A successful response without object data is accepted only for the small mutation endpoint set identified in the client. HTTP success and business success are separate checks. See [envelope handling][client-envelope].

The current adapter consumes ordered JSON `GetImageIndex.images`; it contains no implemented protobuf image-index fallback. A legacy protobuf decoder is not established by this source snapshot. Version markers, pinned assets, and protected response formats are observations from this commit, not a forward compatibility guarantee.

## Account and session observations

Authentication uses form encoding for POST bodies and a separate host allowlist. The source does not follow authentication redirects automatically. See [authentication transport][auth-transport].

| Operation | Method and origin/path | Parameters and consumed response fields |
| --- | --- | --- |
| Generate login QR | `GET https://passport.bilibili.com/x/passport-login/web/qrcode/generate` | Query `source=main_web`, `go_url=https://manga.bilibili.com/`; consumes `data.url` and `data.qrcode_key`. The returned URL is used as the QR payload. |
| Poll login QR | `GET https://passport.bilibili.com/x/passport-login/web/qrcode/poll` | Query `qrcode_key`, `source=main_web`; status is `data.code`, distinct from the envelope code. |
| Initialize manga device context | `GET https://manga.bilibili.com/ductape/buvid` | Source sends no account cookies and captures the official `buvid3` Set-Cookie only. |
| Validate identity | `GET https://api.bilibili.com/x/web-interface/nav` | Requires an actual logged-in response with `data.isLogin=true`; the account namespace derives from the confirmed identity, not an imported user ID alone. |
| Inspect renewal state | `GET https://passport.bilibili.com/x/passport-login/web/cookie/info` | Query `csrf` from `bili_jct`; consumes boolean `data.refresh` and integer `data.timestamp`. |
| Obtain renewal challenge | `GET https://www.bilibili.com/correspond/1/{ciphertext}` | Ciphertext is derived from the server timestamp; parser looks for the refresh CSRF value in `div#1-name`. |
| Refresh cookies | `POST https://passport.bilibili.com/x/passport-login/web/cookie/refresh` | Form fields `csrf`, `refresh_csrf`, `source=main_web`, `refresh_token`; consumes replacement cookies and a replacement refresh token. |
| Confirm previous-token retirement | `POST https://passport.bilibili.com/x/passport-login/web/confirm/refresh` | Form fields `csrf` from the new session and `refresh_token` from the previous session. The source calls this only after durable replacement-session storage. |

QR polling statuses are `86101` for waiting, `86090` for scanned but awaiting confirmation, `86038` for expired, and `0` for confirmed. The success path requires a refresh token and newly received `SESSDATA` and `bili_jct`, initializes manga device context, and validates identity before publishing the session. The server status owns QR expiry; the source does not infer a fixed lifetime from envelope `ttl`. See [QR handling][auth-qr] and [device initialization][site-context].

Supported imports include a Cookie header, browser JSON exports, and Netscape cookie exports, bounded to 128 KiB. Retained cookie names are `SESSDATA`, `bili_jct`, `DedeUserID`, `DedeUserID__ckMd5`, `buvid3`, `buvid4`, `buvid_fp`, `b_nut`, `b_lsid`, and `XSRF-TOKEN`. Domain matching is limited to the supported Bilibili domains. `refresh_token` is separate from the Cookie header. The session implementation tracks pending renewal state and credential generation; its private serialized form differs from its safe UI summary. See [session parsing and fields][session] and [renewal sequencing][auth-refresh].

The plugin persists credentials through private, atomically replaced files. This is a source behavior; its filesystem permissions and storage ownership assumptions have not been transferred to the new application's deployment environment. See [session persistence][session-save].

## Catalog and reading endpoints

All paths in the following table use the manga origin and common POST query above unless a full URL is shown. `comic.v1.Comic` and `bookshelf.v1.Bookshelf` below are Twirp service names. The client bounds ordinary pagination to a page number of at least 1 and a page size of 1 through 100, defaulting to 20.

| Client operation | Service/endpoint | Observed request body | Consumed or normalized result |
| --- | --- | --- | --- |
| Favorites | `bookshelf.v1.Bookshelf/ListFavorite` | `page_num`, `page_size`, `order` default 1, `wait_free=0`, `time_limit_free=0`, `type=0`, `from=web`, `source=web` | Normalized comic array; requires a session in the adapter. |
| History | `bookshelf.v1.Bookshelf/ListHistory` | `page_num`, `page_size`, `type=0` | Normalized comic array; requires a session. |
| Search | `comic.v1.Comic/Search` | `key_word`, `page_num`, `page_size` | Normalized comic array. |
| Comic detail | `comic.v1.Comic/ComicDetail` | Numeric `comic_id`, prepared `m2` | Comic metadata plus `ep_list` or `episodes`, normalized and sorted by fractional `ord`/`order` and ID. |
| Image index | `comic.v1.Comic/GetImageIndex` | Numeric `ep_id`, prepared `m2`; optional preparation query fields are separately supported | Ordered `images` with `path` and geometry `x`/`y` or `width`/`height`. |
| Image tokens | `comic.v1.Comic/ImageToken` | `urls` is a JSON-encoded array stored inside a JSON string field; `m1` is the ephemeral public key | `tokens` or a directly returned array; expected to match request count and order. The client bounds each call to 1 through 20 paths. |
| Write chapter history | `bookshelf.v1.Bookshelf/AddHistory` | Numeric `comic_id`, numeric `ep_id` | Mutation result; no source-page position is transmitted. |
| Follow/unfollow | `bookshelf.v1.Bookshelf/AddFavorite` or `DeleteFavorite` | `comic_ids` is a string | Explicit account mutation. |

Source locations: [favorites, history, search and detail][client-catalog], [index and tokens][client-images], and [history/favorite mutations][client-mutations]. IDs are validated as bounded positive decimal identifiers before numeric JSON conversion in this adapter; their domain and display representation should not be confused with chapter ordinals.

The comic normalizer consumes aliases such as `comic_id`/`id`, `vertical_cover`/`vcover`/`cover`, and `latest_ep_id`. Episode normalization preserves fractional order and distinguishes `free`, `owned`, `temporary`, `locked`, `unavailable`, and `unknown`. Explicit locked state has precedence, and the exact expiry value `0000-00-00 00:00:00` means no expiry. An unlocked flag alone does not establish permanent ownership. See [normalization][normalize].

Additional discovery sources exist in the plugin:

- Recommendations: `GET https://manga.bilibili.com/index.pageContext.json`; parses current page-context recommendation structures rather than calling an assumed recommendation Twirp endpoint. See [recommendations][recommendations].
- Category metadata: `POST /twirp/comic.v1.Comic/AllLabel` with `{}`; consumes `styles` and `orders`. See [category metadata][categories-meta].
- Category results: signed `POST /twirp/comic.v1.Comic/ClassPage` with `style_id`, `area_id=-1`, `is_finish=-1`, `order`, `special_tag=0`, `page_num`, `page_size`, `is_free=-1`, and prepared `m2`. This code creates a bounded guest context with an official `buvid3`; it does not reuse account cookies for this request. See [category page construction][categories-page].

## Signing, response decoding, and image acquisition

### Native signing resources

The plugin's signer is a C host using wasm3 to execute a pinned official Go/WASM module. Its callback is `y1_z2w2a3(sign_query, serialized_body, timestamp_ms)`. The returned sign becomes `ultra_sign`; the adapter supplies `x-bili-data-sn=1E74C20E5720FBF3BB351965D7A9DFC1`. Signing is separate from encrypting a request body. See [signing adapter][native-sign].

The pinned signing asset is `efae82c96a7eef44bee5.wasm`, SHA-256 `39bc0676953752c461197df592e1f5894f1a7492a29400c946e560fc109a8e2e`. The asset manifest also identifies a response-decoding WASM, while the production backend currently calls the local response algorithm described below. Assets are acquired outside account state, checked by digest, and atomically published. These filenames and hashes are snapshot-specific. See [asset manifest and acquisition][assets].

The native interface exposes `biliwasm_run(wasm_path, request_json, output_json)` and `biliwasm_free(output_json)`. Calls are synchronous and serialized in the plugin worker, and every non-null output is released even on failure. The upstream host caps WASM linear memory at 64 MiB; that limit is not a measured total application memory budget. Dependencies and notices include wasm3, cJSON, and the Go host ABI reference. See [native host interface and dependencies][native-readme].

The current `linux-armhf` artifacts are Linux ARMv6 hard-float/glibc builds; their manifests explicitly retain `device_verified=false`. They cannot be treated as compatible static dependencies of a musl application merely because both target ARM. Toolchain, libc, float ABI, linker layout, and physical Kindle compatibility remain integration questions. See [signing artifact manifest][native-manifest], [portable crypto manifest][portable-manifest], and [ARM compatibility evidence][arm-evidence].

### Protected responses and `m2`

The current local response algorithm derives an AES-192 key from alternating bytes in the first 36 ASCII bytes of `buvid`, the `web` platform string, and the last three characters of a selected request field, padded with `=` to 24 bytes. Field selection is `comic_id` for detail, `ep_id` for image index, `style_id` for category pages, and the string-valued `urls` field for image tokens. Base64 `bytesData` is decoded with CBC, an IV from the key's first 16 bytes, and strict padding/JSON checks. This request/response context is essential; using unrelated cookies or reinterpreting the nested `urls` string changes the key. See [response field map and key derivation][response-crypto].

The production backend advertises `index_challenge=false` and `index_error_reporting=true`. It supplies an actual browser-environment error in `m2=error:..._<milliseconds>` rather than implementing a DOM/canvas browser fingerprint. This is the source implementation, not a general guarantee that the server will accept the same path for a different client. See [backend capabilities and preparation][native-preparation].

### Ordered images and transient keys

The index adapter hashes each source path to form an image ID and hashes the ordered path hashes plus geometry to form an acquisition revision. Those hashes describe this index snapshot; they are not proof that remotely changed image bytes are identical. The plugin separates immutable episode descriptors from mutable acquisition state containing paths and temporary URLs. See [index construction][client-images] and [storage contracts][storage-doc].

`m1` contains Base64 of an uncompressed P-256 public key. The private key remains in transient worker context; ECDH yields a raw 32-byte shared x-coordinate. `imageTokens()` stores this context on the client and the corresponding encrypted `downloadImage()` consumes it on that same client/worker. Concurrent token batches cannot safely overwrite a shared client context while earlier encrypted downloads still depend on it. See [key exchange][ecdh] and [token/download coupling][client-download].

Image URL handling is sensitive to the returned field:

- A nonempty `complete_url` is used directly. In the non-HTTP-upgrade branch, the source adds `&code=DanmakuInfo` without re-encoding existing signed parameters. The `http://` branch upgrades to HTTPS and returns that upgraded URL.
- Otherwise, `url` and an escaped `token` form the acquisition URL. Protocol-relative URLs become HTTPS.
- Downloads accept only the supported HTTPS Bilibili image host suffixes. CDN requests receive manga `Referer` and User-Agent, not account cookies.
- The field is spelled `hit_encrpyt` in the upstream response handling. A true value can still lead to an ordinary HTTP `image/*` payload; the backend recognizes that before encrypted-container conversion.

See [URL/download construction][client-download] and [plain-image response handling][native-image]. The source inspects the final image container, dimensions, byte count, and SHA-256. Its `verification="container"` does not mean every pixel was decoded.

The converter implements versions 3, 5, 6, 7, and 8, rejects unknown versions, and bounds encrypted preparation to 16 MiB. Versions 3/7 use AES-256-CTR with a 64-bit counter; version 8 uses a 128-bit counter; these paths transform a bounded prefix. Version 6 uses a CBC prefix with padding, and version 5 uses PBKDF2-SHA512 plus authenticated GCM. Version-specific `cpx`, public-key metadata, version-8 timestamp/scattered metadata, and untouched tail handling are part of the implementation. These details are not interchangeable generic AES settings. See [image converter][image-crypto].

## Persistence, errors, and optional write endpoints

Local page progress and cloud chapter history have different precision. `AddHistory` transmits only a comic/episode pair. The plugin's local anchor includes page ID/index, source coordinates, normalized position, reading mode, and zoom-related state, bound to episode and revision in storage. Its SQLite model also records pages, jobs, descriptors, pins, and page-commit journals. See [local anchors][anchors], [schema][schema], and [history request][client-mutations]. No page-level cloud sync endpoint was found in the reviewed client.

The transport defaults to a 15-second socket timeout, 60-second total timeout, 8 MiB JSON limit, and 64 MiB image limit, with request-specific overrides. It verifies both TLS certificate chains and hostnames and disables automatic redirects. These are upstream transport observations. See [transport][transport]. Credentials, signed URLs, private keys, and token payloads are excluded from ordinary catalog extras; see [sanitization][normalize].

The error adapter distinguishes authentication rejection (`-101` or `401`), request-authentication rejection (`-111`), and image-index access errors (`1` for locked and `501` for unavailable). HTTP `429` and server `5xx` responses can be marked retryable. A generic image `403` is not treated as proof of token expiry and does not trigger blind token refresh. Actual purchase submission has no automatic retry wrapper or documented idempotency key in this client. An unknown response or response loss is retained as uncertain. See [errors][errors], [download errors][client-download], and [purchase submission][client-purchase].

The following optional account/payment endpoints are source references only. None was executed for this handoff, and their presence does not establish product scope or authorization to transact.

| Service/endpoint | Observed request or interpretation |
| --- | --- |
| `user.v1.User/GetWallet` | Empty JSON object; returns wallet data. |
| `comic.v1.Comic/GetEpisodeBuyInfo` | `ep_id`; scoped variants add `buy_type`, `batch_limit`, `order`, and the bare `getEpisodeDiscounts` query flag. The adapter does not transmit `start_ord` in this quote body. |
| `comic.v1.Comic/GetDiscountList` | `comic_id`, `order`, `original_values`. |
| `comic.v1.Comic/CalDiscountPrice` | `id`, `original_values`. |
| `comic.v1.Comic/GetComicFreeGoldCard` | `comic_id`, `ep_id`, `buy_type`, `batch_limit`. |
| `comic.v1.Comic/BuyEpisode` | Explicit coin/coupon purchase. Single scope uses `ep_id`; the supported ordinal coin range uses `comic_id`, `with_ord_scope=true`, `start_ord`, `limit`. Only an allowlist of price/coupon/card fields is accepted. `buy_method=2` denotes coupons and `3` denotes currency in this adapter. |
| `pay.v1.Pay/GetPayConfig` | Empty JSON object; source consumes offered amount ranges. |
| `pay.v1.Pay/CreateOrder` | `pay_type=qr`, `pay_amount` in integer cents, after current-config validation in the adapter. |
| `user.v1.User/GetPayOrders` | Bounded history options; source retains exact order-ID handling. |

See [wallet and quote operations][client-quotes], [purchase payload checks][client-purchase], and [recharge adapter][recharge]. Recognized purchase rejection codes 1 through 5 are treated as definitive by the source; unknown codes are not. Upstream ordinal quote evidence explicitly retains `server_confirmed_ids=false`; constructed intended episode sets are not server purchase receipts.

## Evidence and unresolved integration points

The newer [upstream implementation status][implementation-status] and [finishing acceptance][finishing-acceptance] report real QR confirmation, device-context initialization, private persistence/restart, authenticated catalog/index/token requests, and a complete 45-page online/download/offline workflow. They also report earlier already-owned image samples and synthetic encrypted-container checks. These are existing project records, not reruns or independent verification in this task.

Some older protocol notes still say that no successful login or image acquisition had occurred. Those statements describe an earlier snapshot. The later records report that missing `buvid3` caused protected-catalog failure for fresh QR sessions and that official site initialization repaired that observed workflow. The same records retain limits: no physical Scribe execution, actual charging, credential rotation/old-token confirmation, or live encrypted-container wire was established. Passing ordinary real JPEG downloads does not prove a live encrypted-container path.

For the new application, the following remain unresolved observations rather than prescribed implementation choices: compatibility of the pinned signing host and crypto with the chosen Kindle toolchain; target libc/float ABI and memory behavior; continuing acceptance of the snapshot's request/version fields and `m2` path; future protected-response or protobuf-related format changes; complete credential renewal; and source/dependency licensing. No top-level license grant was found in the reviewed repository. Third-party notices and public asset URLs do not by themselves establish permission to redistribute all upstream code or official WASM assets.

[upstream]: https://github.com/moooyo/bilicomics.koplugin/tree/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c
[client-headers]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/client.lua#L72-L89
[client-envelope]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/client.lua#L101-L139
[client-post]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/client.lua#L141-L180
[client-catalog]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/client.lua#L183-L261
[client-images]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/client.lua#L263-L313
[client-download]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/client.lua#L315-L363
[client-quotes]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/client.lua#L365-L467
[client-purchase]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/client.lua#L469-L502
[client-mutations]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/client.lua#L504-L514
[auth-transport]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/auth.lua#L59-L144
[auth-qr]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/auth.lua#L146-L194
[auth-refresh]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/auth.lua#L212-L339
[site-context]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/site_context.lua#L4-L77
[session]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/session.lua#L8-L269
[session-save]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/session.lua#L272
[normalize]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/normalize.lua#L3-L118
[recommendations]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/recommendations.lua#L5-L111
[categories-meta]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/categories.lua#L93-L120
[categories-page]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/categories.lua#L198-L230
[native-sign]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/native_backend.lua#L88-L103
[native-preparation]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/native_backend.lua#L47-L117
[native-image]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/native_backend.lua#L119-L189
[native-readme]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/native/README.md#L1-L109
[native-manifest]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/native/manifest.json#L50
[portable-manifest]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/native/portable/manifest.json#L40
[arm-evidence]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/docs/kindle-native-compatibility-fix.md
[assets]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/assets.lua#L5-L81
[response-crypto]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/response_crypto.lua#L8-L216
[ecdh]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/ecdh.lua#L184
[image-crypto]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/image_crypto.lua#L8
[storage-doc]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/docs/development-contracts.md
[anchors]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/reader/anchors.lua#L8-L33
[schema]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/storage/migrations.lua#L1-L20
[transport]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/transport.lua#L33-L131
[errors]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/errors.lua#L16-L43
[recharge]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/bilicomics/protocol/recharge.lua#L292-L335
[implementation-status]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/docs/implementation-status.md#L3-L79
[finishing-acceptance]: https://github.com/moooyo/bilicomics.koplugin/blob/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/docs/finishing-acceptance.md
