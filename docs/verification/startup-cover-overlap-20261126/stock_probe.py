"""Check only the installed inputs of the Scribe route guard under a fresh lease."""

import argparse
import getpass
import json
import os
from pathlib import Path
import re
import signal
import sys
import uuid
import warnings

from compare import BASE, CONTROL, PRODUCTION_BYTES, ROUTE, guard, interrupted, require


# Device output is limited to pass/fail flags and recognized document forms.
# Neither file contents nor unrelated catalog rows leave the device.
DEVICE_PROBE = r'''export LC_ALL=C
media=/mnt/us
home=$media/bilicomics
doc=$media/documents/bilicomics.sh
plain() { [ ! -L "$1" ] && [ -f "$1" ] && [ "$(stat -c '%u:%h' "$1")" = 0:1 ]; }
directory() { [ ! -L "$1" ] && [ -d "$1" ] && [ "$(readlink -f "$1")" = "$1" ]; }
check() {
    label=$1; shift
    if "$@"; then printf '%s=yes\n' "$label"; else printf '%s=no\n' "$label"; fi
}
check media directory "$media"
check documents directory "$media/documents"
check home directory "$home"
check app directory "$home/app"
check data directory "$home/data"
check document_is_plain plain "$doc"
legacy_card() { plain "$doc" && cmp -s - "$doc" <<'BILI_LEGACY_CARD'
@LEGACY_CARD@BILI_LEGACY_CARD
}
icon_card() { plain "$doc" && cmp -s - "$doc" <<'BILI_ICON_CARD'
@ICON_CARD@BILI_ICON_CARD
}
check document_matches_legacy legacy_card
check document_matches_icon icon_card
check launcher_is_plain plain "$home/launch.sh"
launcher_matches() { plain "$home/launch.sh" && cmp -s - "$home/launch.sh" <<'BILI_SHIPPING_LAUNCHER'
@SHIPPING@BILI_SHIPPING_LAUNCHER
}
check launcher_matches launcher_matches
card_inventory() (
    seen=0
    for path in "$media"/documents/[Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].*; do
        [ -e "$path" ] || [ -L "$path" ] || continue
        case "${path##*/}" in
            [Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Hh])
                [ "$path" = "$doc" ] || return 1
                seen=$((seen + 1));;
            [Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Dd][Rr]|[Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Hh].[Ss][Dd][Rr])
                return 1;;
        esac
    done
    [ "$seen" -eq 1 ]
)
check document_inventory_passes card_inventory
one_card_without_counting_metadata() (
    seen=0
    for path in "$media"/documents/[Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].*; do
        [ -e "$path" ] || [ -L "$path" ] || continue
        case "${path##*/}" in
            [Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Hh])
                [ "$path" = "$doc" ] || return 1
                seen=$((seen + 1));;
        esac
    done
    [ "$seen" -eq 1 ]
)
check document_is_only_scriptlet one_card_without_counting_metadata
check direct_sidecar_directory directory "$media/documents/bilicomics.sdr"
check scriptlet_sidecar_directory directory "$doc.sdr"
check upper_sidecar_lookup directory "$media/documents/BiliComics.sdr"
check upper_scriptlet_sidecar_lookup directory "$media/documents/BiliComics.sh.sdr"
standard_sidecars_only() (
    for path in "$media"/documents/[Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].*; do
        [ -e "$path" ] || [ -L "$path" ] || continue
        case "${path##*/}" in
            bilicomics.sdr|bilicomics.sh.sdr) directory "$path" || return 1;;
            [Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Dd][Rr]|[Bb][Ii][Ll][Ii][Cc][Oo][Mm][Ii][Cc][Ss].[Ss][Hh].[Ss][Dd][Rr])
                return 1;;
        esac
    done
)
check standard_sidecars_only standard_sidecars_only
catalog_matches() {
    rows=$(sqlite3 -readonly -cmd '.timeout 1500' /var/local/cc.db "SELECT count(*)||':'||coalesce(sum(p_location COLLATE BINARY='$doc' AND p_mimeType='text/x-shellscript' AND p_titles_0_nominal='BiliComics for Kindle'),0) FROM Entries WHERE p_location COLLATE NOCASE='$doc';") && [ "$rows" = 1:1 ]
}
check catalog_matches catalog_matches
check expected_executable_size test "$(stat -c %s "$home/app/bilicomics")" = @APP_SIZE@
check expected_worker_size test "$(stat -c %s "$home/app/kindle-jpeg-worker")" = 426672
check executable_is_plain plain "$home/app/bilicomics"
check worker_is_plain plain "$home/app/kindle-jpeg-worker"
profile_matches() { plain "$home/data/device-profile" && printf 'scribe\n' | cmp -s - "$home/data/device-profile"; }
allowlist_matches() { plain "$home/data/device-allowlist" && printf 'scribe 5.19.5\n' | cmp -s - "$home/data/device-allowlist"; }
check profile_matches profile_matches
check profile_is_plain plain "$home/data/device-profile"
check allowlist_matches allowlist_matches
check allowlist_is_plain plain "$home/data/device-allowlist"
'''


def probe_script(shipping, icon):
    inputs = {'APP_SIZE': str(PRODUCTION_BYTES)}
    for key, delimiter, data in (
        ('LEGACY_CARD', 'BILI_LEGACY_CARD', ROUTE.DOCUMENT),
        ('ICON_CARD', 'BILI_ICON_CARD', icon),
        ('SHIPPING', 'BILI_SHIPPING_LAUNCHER', shipping),
    ):
        text = data.decode('ascii')
        require(text.startswith('#!/bin/sh\n') and text.endswith('\n') and '\r' not in text
                and '\0' not in text and delimiter not in text, 'Unexpected reference script')
        inputs[key] = text
    return re.sub(r'@([A-Z_]+)@', lambda match: inputs[match[1]], DEVICE_PROBE).encode('ascii')


def parse_report(data):
    fields = re.findall(r'^check ([a-z_]+) ', DEVICE_PROBE, re.MULTILINE)
    require(len(data) <= 2048, 'Oversized stock observation')
    rows = data.decode('ascii').splitlines()
    require(len(rows) == len(fields), 'Incomplete stock observation')
    result = {}
    for key, row in zip(fields, rows):
        require(row in (f'{key}=yes', f'{key}=no'), 'Unexpected stock observation field')
        result[key] = row.endswith('=yes')
    return result


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('shipping-launcher', 'icon-document', 'rollout-root'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    owner, boot = CONTROL.identity()
    script = probe_script(CONTROL.plain(args.shipping_launcher, 200_000).read_bytes(),
                          CONTROL.plain(args.icon_document, 1024).read_bytes())
    run = 'bili-shelf-' + uuid.uuid4().hex
    private = BASE.batch_path(run)
    private.mkdir(mode=0o700)
    CONTROL.save(private / 'admission.json', {'run': run, 'owner': owner, 'host_boot': boot,
                                            'rollout_root': str(args.rollout_root)})
    CONTROL.write_once(private / 'native-guard.sh', guard())
    CONTROL.write_once(private / 'askpass', CONTROL.ASKPASS)
    (private / 'askpass').chmod(0o700)
    visit = CONTROL.Operator(private)
    require(sys.stdin.isatty(), 'Expected a private terminal')
    signal.signal(signal.SIGHUP, interrupted)
    signal.signal(signal.SIGTERM, interrupted)
    with warnings.catch_warnings():
        warnings.simplefilter('error', getpass.GetPassWarning)
        visit.password = getpass.getpass('Kindle authentication for this read-only audit (not saved): ')
    require(visit.password, 'No authentication provided')
    CONTROL.announce(f'Read-only audit batch: {private}')
    first_native = final_native = False
    try:
        try:
            visit.lease.acquire(visit.request)
        except visit.lease.LeaseError as error:
            if error.code in ('lease_exists', 'operation_busy') and not any(error.details.values()):
                (private / 'askpass').unlink()
                raise CONTROL.OperatorError('Shared lease busy; no device contact, reservation or retry') from None
            raise CONTROL.RecoveryRequired('Audit lease acquisition incomplete; no device contact') from None
        try:
            visit.boot = visit.guard('native')
            require(CONTROL.UUID.fullmatch(visit.boot), 'The Kindle did not report native idle')
            first_native = True
            observation = visit.raw('sh -s', script, deadline=25)
        finally:
            try:
                final_native = first_native and visit.guard('native') == visit.boot
            except (OSError, ValueError, CONTROL.subprocess.SubprocessError):
                final_native = False
            try:
                # This audit opens no app, writes no files and changes no native state.
                receipt = visit.lease.release(visit.request, native_restored_and_connections_closed=True)
            except visit.lease.LeaseError:
                raise CONTROL.RecoveryRequired('Exact audit lease release failed') from None
            public_release = {key: receipt[key] for key in (
                'state', 'owner_thread', 'run_id', 'owner_and_token_checked', 'native_restored_and_connections_closed')}
            public_release['device_disposition'] = 'native-idle-confirmed' if final_native else 'no-device-change'
            CONTROL.save(private / 'released.json', public_release)
            (private / 'private-token').unlink()
            (private / 'askpass').unlink()
            CONTROL.announce('Read-only audit closed; exact lease released.')
    finally:
        visit.password = ''
    require(final_native, 'The same native idle state was not confirmed after the audit')
    result = {'schema': 'bili-cover-probe-stock-v1', 'read_only': True, 'native_idle_same_boot': True,
              'lease_released': True, **parse_report(observation)}
    CONTROL.save(private / 'stock-audit.json', result)
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    try:
        main()
    except CONTROL.OperatorError as error:
        sys.exit(str(error))
    except (OSError, ValueError, CONTROL.subprocess.SubprocessError, KeyboardInterrupt):
        sys.exit('Stock audit did not complete; check the original batch receipt.')
