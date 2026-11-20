# Independent native card-click research runner

This runner is for the rebuilt `test-env` and its shared Scribe. It does not
reconstruct the missing R7 click workspace, use its historical archive as an
input, or change the shipping application. Both rebuilt packages must already
have passed the [independent software gates](../../../../verification/startup-rebuild-20261120/README.md).

## Preparation without the Kindle

Use the shared `ubuntu-run` environment, with `--build` only for compilation,
and private project directories. Never acquire the physical lease for these steps.

1. Copy the [as-tested standalone selector runner and fixtures](../../../library-pairs-20261120/selector/)
   and the historical `../pair/pair-fs.c` and `../pair/select-arms.in.sh` to
   a private directory. Qualify `pair-fs` with `sh build-selector.sh --standalone`. Build the
   existing `scripts/verification/physical_framebuffer.c` as a static ARMv7
   musl reader with the shared Zig compiler.
2. The target recognizer uses Ubuntu Noble's **ARM hard-float** `libX11`,
   `libXtst`, `libtesseract5` and its runtime dependencies, and the English and
   Simplified Chinese trained data. Obtain the distribution packages using
   signed Ubuntu Ports package indexes in an isolated APT state, extract
   them into `runtime/`, and extract `libx11-dev`, `libxtst-dev`,
   `x11proto-dev` and `libtesseract-dev` headers into `sdk/` using
   `fetch-runtime.sh`. The private extracted distro tree retains the upstream
   package notices. No system package installation on the Kindle or on the
   worker host is involved.
3. Use `build-target.sh` in the recognizer workspace, then run
   `BILI_CLICK_FIXTURE=1 xvfb-run -a -s '-screen 0 900x1200x24' python3 -B test-native-target.py`.
   The actual ARM programs execute under QEMU: a unique full title is eligible,
   an ambiguous, partial or extended title is not. The `native-target` binary
   cannot send input. The separate `card-input` binary only clicks positively
   identified public navigation or the full target title. Immediately before
   pressing, it compares the corresponding pixels and both navigation labels
   with the OCR frame. The recorded down and up brackets use `CLOCK_MONOTONIC`
   and XSync; the hold interval is 50 ms.
4. `pack-kit.py` builds a navigation-only kit; `--paired` includes the
   input-capable program and the loader's actual shared libraries. Bundle no
   diagnostic binary, private screen image, account data or credentials.
5. On the host, forward the actual `CODEX_THREAD_ID` into a fresh private
   workspace with `prepare-pair.py`, `device-pair.sh`, `run-pair.py`,
   `device_lease.py` and `native-inventory.sh`. Use the freshly qualified A and
   B directories, a closed desktop **demo-only** seed, the paired kit and
   framebuffer reader. First prepare `--same-package`
   and run `simulate-pair.py` on that dry batch with `BILI_CLICK_FIXTURE=1` in
   `ubuntu-run`. The rehearsal uses the real install/selection/removal logic in
   a temporary filesystem and refuses to delete a foreign replacement document.
   Prepare a **different** real batch without `--same-package`.

## Device operation and recovery

`read-inventory.py` only reads the narrow public state. The independent
`navigation-trial.py` can validate on-device recognition without deploying
the application or issuing input. Both run from private directories on the
host, using its pinned `kindle-physical` SSH alias and exact shared lease.
Do not run a device controller inside `ubuntu-run`: it has a different `/tmp`.

`run-pair.py --batch <new-host-batch> --expected-host <authorized-address>`
owns one continuous operation. Admission binds the boot, owner, run, variant
order and inputs. The password is prompted before lease acquisition, used by
SSH askpass and not stored. Native state and original canonical absence are
rechecked. Only the exact index entry for `BiliComics for Kindle` may be
selected; neither URI dispatch nor fixed-coordinate guessing is a fallback.
The instrumented launchers keep real `--library` admission, but direct each
process to fresh copies of the same offline demonstration data. Both variants
are staged before timing. On this Kindle the media filesystem does **not**
support the kernel's no-replace directory move. Do not substitute `mv` onto
the canonical path. Instead the v2 controller creates the previously absent
canonical root once using exclusive `mkdir`. The common immutable entry there
executes the selected production launcher with `--library`; selection changes
only in an explicitly mode-0700 private directory. Each case permits one
dispatch. The router and both selected program leaves are checked against
run-owned reference copies; expensive comparisons are outside the click
bracket. The common shell hop is part of the measured instrumented route.

All warmups and six balanced pairs are retained, including any failed
attempt. A bounded independent guard may only signal the matching worker and
invalidates that case if used. Before release, the controller verifies native
control, removes the owned canonical entry/install, checks selected protected
files and handler configuration, restores the original tab and pointer, and
removes staging. If recovery is interrupted, use `--recover` **only** with
that existing batch and its still-owned lease; never rebind it to a new one.
`report-pair.py` runs on closed evidence and imports the original
`library-to-shelf-20261119/project_archived.py` timing definition. It reports
all six paired differences for each endpoint, not optical settling.

The [November 21 private-render study](../../../shelf-render-20261121/README.md)
uses `prepare-pair.py --shelf-render`. This protocol enforces the previously
qualified November 20 candidate as control and accepts only the new software-
qualified ARM application as a changed package member. Both arms collect the
bounded pre-takeover `launcher-stages.tsv`; only the new arm's complete Shelf
is expected to render before waiting. The default November 20 protocol and
the archived intervals retain their original ordering checks.
