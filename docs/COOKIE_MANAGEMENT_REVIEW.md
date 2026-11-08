# Cookie Management Review

Reviewed on 2026-11-04 (Asia/Shanghai).

## Findings and changes

The production client already constructed explicit Cookie headers for manga and
authentication requests. QR generation and polling, device initialization, public
category metadata, image/CDN downloads, and signing-asset requests intentionally
exclude account cookies. This review did not use a real account or change that
separation.

The review identified and corrected the following defects:

- Ordinary response cookies changed only shared memory. The active application
  client now opts into `SessionFile` persistence, with storage and publication
  serialized by the session lock. Isolated import candidates do not write that
  store. Logout and authentication rejection clear the same durable state.
- Cookie scope and lifetime were incomplete. Schema 3 preserves host-only flags
  and per-cookie expiration. Browser JSON and Netscape imports retain supported
  scope information; expired cookies are excluded. Schema 1 and 2 have an explicit
  migration, and an expired saved session no longer prevents application startup.
- XSRF, CSRF, and signing device values were read without the destination's scope
  filter. Cookie headers and auxiliary values now use the same domain and expiry
  selection. Account-required requests fail before transport if their destination
  cannot see the session cookie.
- Cookies from rejected responses were dropped. Active-session response handling
  now processes supported Set-Cookie fields even when HTTP or business status
  rejects a non-authentication operation. An authentication or identity rejection
  cannot publish unverified replacement credentials; its captured session is
  conditionally invalidated instead. QR/import candidates remain isolated from
  active state.
- Concurrent responses could discard independent updates, and delayed renewal
  callbacks could overwrite newer cookies. Independent cookie updates now merge
  against their captured values. Credential and account replacements retire old
  response contexts. Renewal and confirmation activate under the precise source
  epoch on the worker, before the UI receives a result.
- Queued favorite mutations could acquire another account's credentials, while
  reader history used a separate client. Both now bind the shared client to the
  selected account before scheduling work.
- A directory-sync error after publication left old RAM able to overwrite a newly
  published file. That outcome now closes authenticated admission and persistence
  until an explicit new login or logout completes storage recovery. It remains an
  uncertain storage result. A new QR candidate can still be validated through an
  isolated client without reusing the fenced credentials.

An adjacent request-outcome defect was also corrected: a decoder error after a
purchase/order response cannot claim that the request was never transmitted.
Such a response remains an unknown transaction outcome and does not authorize an
automatic retry.

## Supported cookie boundary

This is a bounded service adapter, not a general browser cookie jar. It retains
the reviewed cookie-name and Bilibili-origin allowlists and supports root-path
cookies. A missing response Path uses the actual request's default directory;
unsupported non-root paths are excluded. Same-name cookies from different scopes
are explicitly rejected rather than silently broadened or overwritten. A deletion
only removes the matching retained scope.

Cookie values are preserved as supplied. SESSDATA is not URL-decoded and encoded
separators are not reserialized. Only the manga XSRF auxiliary header is percent
decoded, with invalid header material rejected. Refresh tokens remain separate
from Cookie headers and debug output.

## Verification

The final isolated `test-env` run passed:

| Check | Result |
| --- | --- |
| `cargo check --locked --offline` | Passed without warnings |
| `cargo test --locked --offline --lib` | 197 passed, 0 failed, 0 ignored |
| `cargo build --locked --offline --bin bilicomics` | Passed without warnings |
| Controlled production-service subprocess integration | 2 passed, 0 failed, 0 ignored |

The library includes 15 focused cookie-request regressions plus session-scope,
renewal, failure-injection, and application-startup cases. Coverage includes an
actual temporary-file rename followed by an injected directory-sync failure,
shared-client fencing, a new QR login through that fence, explicit logout
recovery, independent concurrent updates, delayed responses, request Cookie/CSRF
agreement, and purchase/order decoder failures after transmission. Transports and
credentials in these cases are synthetic; request-header assertions observe the
production client at its transport boundary rather than a live Bilibili account.

The production-service integration retained its separate evidence class. It
verified process termination after a committed page, missing-page recovery,
corrupt-receipt repair, offline raster/anchor continuity, and two preparation
restarts preserving unknown transaction intent without increasing purchase
attempts. It used an injected signer and synthetic image payloads inside an
isolated Linux network namespace. It did not establish authenticated UI clicks,
real service compatibility, or full payment reconciliation.

Final evidence is under
`artifacts/verification/cookie-review-20261104/`. The `r3` core summary and the
`production-flow` result are authoritative for this source. Their manifest
serialization order differs, but a remote comparison confirmed equality of all
335 source path/hash pairs. The core manifest SHA-256 is
`a2ce7fd83a447bdfb4a22d9fdd7263505d0b8eed14af3c621bbf6b17af708382`.
The sealed desktop executable SHA-256 is
`eff0d6850d4422d4e02fbfe613250524bfbe4bb2a4864700d47a8bb264f0df9b`.

Earlier `r1` and failed `r2` evidence remain intact. The `r2` failure found that
missing CSRF validation could obscure a previously blocked confirmation outcome;
the blocked-state check now runs first. The final regression preserves that
assertion and adds QR recovery coverage rather than relaxing it.

All execution and verification ran through `ssh test-env` with one build job.
No local build/test, real account, real purchase, firmware-emulator run, or physical
Kindle connection was used for this review. The earlier ARM package and its
pre-device evidence remain historical artifacts; they do not contain these cookie
changes. No replacement Kindle package was built in this review.
