# Synthetic protocol fixtures

These public fixtures come from `moooyo/bilicomics.koplugin` revision
`a05ffb987c2089ca389cb3462aafd3ee1c0fa36c`, under `research/protocol/`.
They contain synthetic identifiers, keys and image bytes. They are not account
credentials, real user data, downloaded comics or evidence of live service access.

The upstream research compared their outputs with the unchanged official WASM.
The Rust tests exercise the production response decoder, P-256 agreement,
image-container versions 3/5/6/7/8, CBC padding, authenticated GCM and both CTR
counter widths. Tests must run on `ssh test-env` unless local verification has
been explicitly authorized for the current task. Including the fixtures does
not establish a new test result, device support or live authenticated acceptance.

Source:
[pinned protocol research](https://github.com/moooyo/bilicomics.koplugin/tree/a05ffb987c2089ca389cb3462aafd3ee1c0fa36c/research/protocol).
