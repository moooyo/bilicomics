# Design Source

## Frozen implementation baseline

The user requested preserving the current remote-rendered UI on 2026-11-14.
The [UI/UX baseline index](uiux/README.md) links 51 unchanged Paperwhite 6 and
Scribe captures, full scene indexes, four-fit comparisons, and an offline
viewer from application version 0.1.25. This is the implementation record for
future comparisons; the design reference and subsequent approvals below
retain their own authority and provenance.

## Current reference: v2

On 2026-11-04 the user supplied the v2 self-contained HTML from Downloads and requested an implementation walkthrough and corrections. The 2,831,763-byte file has SHA-256 `62425b5a36df75457fd1769667b5fe88ef06a9d7c6c566a9608a12549aa88414`.

Static extraction yields the same 26 named artboards and 100 embedded resources as the initial reference. Twenty-two templates have meaningful content changes; four differ only by generated resource identifiers. The embedded JavaScript runtime content is unchanged. The v2 design's internal title remains v0.1 and does not select an application release number.

The current review uses the unpacked templates, their state handlers, 57 remote reference renders, and computed element geometry. The initial 52 renders were supplemented with five exact download-drawer and storage states to resolve observed clipping and spacing differences. Generated reference evidence is retained under `artifacts/reference-v2/`. The corrections and evidence are recorded in [the v2 implementation review](DESIGN_REVIEW_V2.md). Native startup activity and retained-reader sleep behavior remain separately authorized requirements.

The geometric demo artwork is reused from the initial reference where its source content is unchanged. V2 presentation data and state labels have their own source identity in `assets/demo/catalog.json`; this does not rewrite the original artwork export provenance.

## Initial reference

The user supplied [the BiliComics Kindle design](https://claude.ai/code/artifact/c7bb1f76-34b0-47b9-bbf7-a45b51e4797d) and subsequently downloaded its self-contained HTML to the operator's computer. That file was statically unpacked without executing its JavaScript locally.

Its SHA-256 is `210e667f40dc6a1c4999b546e1b46111c99a028131cd9951c8de21cdd5a4db57`. It contains 26 artboards: the design foundation, four shared components, and 21 application views across Paperwhite and Scribe, including a landscape spread.

The styles and state handlers are the implementation reference. The application uses native Slint components and Rust controllers; it does not embed the HTML runtime or require React. Numeric values are captured in `assets/design-tokens.json` and the corresponding theme.

Reference rendering ran through `ssh test-env`. The unpacked pages were reconstructed with their local resources and rendered with Chromium at their logical design dimensions. The resulting 57 reference images cover 21 default views and 36 additional states, retained under the ignored `artifacts/reference/` directory with source and render records.

The design contains no image binaries or bundled fonts. Its covers and comic examples are geometric CSS/inline illustrations. Selected illustrations were exported by the remote renderer into `assets/demo/` for the isolated preview. `catalog.json` and `manifest.json` retain their fictional identities and exact display data. Page substitutions and the separate strip illustration are identified as fixtures.

Noto Sans CJK SC and Noto Serif CJK SC are bundled with their notices for offline Chinese rendering. Only required UI and comic-title font weights are included. Prototype timers, fixed balances, QR patterns, and empty event handlers were treated as demonstrations; production controllers use confirmed responses, local state, or an explicit unavailable state.
