#!/usr/bin/env python3
"""Check a frozen UI/UX baseline's inventory and image identities on test-env."""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import struct
import sys


def require(value, message):
    if not value:
        raise ValueError(message)


def load_json(path):
    return json.loads(path.read_text(encoding='utf-8'))


def contained_file(root, relative):
    require(isinstance(relative, str) and relative and '\\' not in relative,
            'Inventory paths must be nonempty portable relative paths')
    name = Path(relative)
    require(not name.is_absolute() and '..' not in name.parts, 'Inventory path escapes the baseline')
    target = (root / name).resolve(strict=True)
    require(target.is_relative_to(root) and target.is_file(), 'Inventory entry is not a contained file')
    return target


def png_size(payload):
    require(len(payload) >= 33 and payload[:8] == b'\x89PNG\r\n\x1a\n'
            and payload[12:16] == b'IHDR', 'Capture is not a PNG with an IHDR header')
    return struct.unpack('>II', payload[16:24])


def verify(root):
    manifest = load_json(root / 'manifest.json')
    require(re.fullmatch(r'[0-9a-f]{40}', manifest['source_commit']), 'Invalid source commit')
    require(re.fullmatch(r'[0-9a-f]{64}', manifest['binary_sha256']), 'Invalid executable identity')
    require(manifest['capture_host'] == 'test-env' and manifest['scale_factor'] == 2,
            'Unexpected capture environment or scale')
    require(manifest['live_account_used'] is False and manifest['physical_kindle_used'] is False,
            'This baseline checker accepts only isolated software-demo captures')

    inventory = manifest['repository_files']
    names = [entry['path'] for entry in inventory]
    require(len(names) == len(set(names)), 'Duplicate frozen inventory entries')
    require('manifest.json' not in names, 'The manifest cannot contain its own checksum')
    actual = {path.relative_to(root).as_posix() for path in root.rglob('*') if path.is_file()}
    require(actual == set(names) | {'manifest.json'}, 'The baseline contains missing or unlisted files')
    for entry in inventory:
        data = contained_file(root, entry['path']).read_bytes()
        require(len(data) == entry['bytes'], 'File size differs: ' + entry['path'])
        require(hashlib.sha256(data).hexdigest() == entry['sha256'], 'File hash differs: ' + entry['path'])

    captures = manifest['captures']
    require(len(captures) == manifest['capture_count'], 'Capture count differs from the manifest')
    identities = {(capture['profile'], capture['id']) for capture in captures}
    require(len(identities) == len(captures), 'Duplicate capture identities')
    counts = Counter(capture['profile'] for capture in captures)
    require(set(counts) == set(manifest['profiles']), 'Profile inventory differs')
    for profile, count in counts.items():
        require(count == manifest['profiles'][profile]['count'], 'Profile capture count differs: ' + profile)

    expected_raw = set()
    for capture in captures:
        profile, scene = capture['profile'], capture['id']
        require(re.fullmatch(r'[a-z0-9-]+', profile) and re.fullmatch(r'\d{2}-[a-z0-9-]+', scene),
                'Invalid capture path component')
        relative = 'raw/' + profile + '/' + scene + '.png'
        expected_raw.add(relative)
        payload = contained_file(root, relative).read_bytes()
        require(hashlib.sha256(payload).hexdigest() == capture['sha256'], 'Source capture hash differs: ' + relative)
        require(png_size(payload) == (capture['width'], capture['height']), 'Capture dimensions differ: ' + relative)
        portrait = tuple(manifest['profiles'][profile]['portrait_pixels'])
        require(png_size(payload) in (portrait, portrait[::-1]), 'Capture is outside its profile geometry')
        metadata = load_json(contained_file(root, relative[:-4] + '.json'))
        for key in ('id', 'profile', 'sha256', 'width', 'height'):
            require(metadata[key] == capture[key], 'State sidecar differs: ' + relative + ' / ' + key)
        require(metadata['file'] == relative, 'State sidecar has a nonportable capture path')
        require(metadata['binary_sha256'] == manifest['binary_sha256'], 'Sidecar executable identity differs')
        require(metadata['demo'] is True and metadata['live_account_used'] is False
                and metadata['physical_device_used'] is False, 'Sidecar scope differs')
        require(metadata['stability']['identical_samples'] >= 3, 'Capture lacks stable-frame evidence')
    actual_raw = {path.relative_to(root).as_posix() for path in (root / 'raw').rglob('*.png')}
    require(actual_raw == expected_raw, 'Native PNG inventory differs from the capture list')
    return {'passed': True, 'source_commit': manifest['source_commit'], 'captures': len(captures),
            'profiles': dict(counts), 'frozen_files': len(actual), 'application_started': False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', type=Path, required=True)
    arguments = parser.parse_args()
    if not sys.platform.startswith('linux'):
        parser.error('Run baseline verification only on remote Linux test-env')
    try:
        result = verify(arguments.baseline.resolve(strict=True))
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(json.dumps({'passed': False, 'error': str(error)}), file=sys.stderr)
        return 1
    print(json.dumps(result))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
