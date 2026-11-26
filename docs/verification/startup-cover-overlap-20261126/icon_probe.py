"""Qualify the public icon on native Library without opening the card."""

import argparse
import re
from pathlib import Path
import sys

from compare import CONTROL, SCHEMA, Visit, interrupted, plan_cases, require


class IconProbe(Visit):
    def measure(self, route_data, probe_data):
        del route_data
        self.step('native-preflight')
        self.boot = self.guard('native')
        require(CONTROL.UUID.fullmatch(self.boot), 'Missing device boot from native preflight')
        CONTROL.save(self.path / 'origin.json', {'boot': self.boot})
        self.step('stage-detector')
        self.raw(f'''space=$(df -Pk /mnt/us | awk 'END {{print $(NF-2)}}')
case "$space" in ''|*[!0-9]*) exit 1;; esac
test "$space" -ge {self.admission['minimum_media_kib']}''')
        self.upload(self.detector, probe_data, 'detector')
        self.step('native-navigation')
        original = self.probe('navigation')
        tab, point = self.shared.paired.selected(original), original['pointer']
        require(tab in ('home', 'library') and type(point) is list and len(point) == 2
                and all(type(value) is int for value in point) and 0 <= point[0] < 1860 and 0 <= point[1] < 2480,
                'Unrecognized starting Library surface')
        CONTROL.save(self.path / 'navigation.json', {'tab': tab, 'pointer': point})
        self.ensure_tab('library')
        counts = []
        for _ in range(2):
            self.step('identify-card')
            card = self.probe('card')
            require(self.shared.paired.selected(card) == 'library' and type(card['title']['count']) is int
                    and 0 <= card['title']['count'] <= 2, 'Invalid public icon observation')
            counts.append(card['title']['count'])
        CONTROL.save(self.path / 'icon-counts.json', {'card_opened': False, 'matches': counts})

    def analyze(self):
        release = CONTROL.load(self.path / 'released.json')
        require(release['state'] == 'released' and release['owner_and_token_checked'] is True
                and release['native_restored_and_connections_closed'] is True, 'Exact lease return not confirmed')
        result = CONTROL.load(self.path / 'icon-counts.json')
        counts = result['matches']
        require(result['card_opened'] is False and len(counts) == 2
                and all(type(count) is int and 0 <= count <= 2 for count in counts), 'Unexpected icon-only result')
        print(f'Public logo matches on successive Library reads: {counts[0]}, {counts[1]}; no card opened.', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--plan', type=Path, required=True)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--start', action='store_true')
    mode.add_argument('--recover', action='store_true')
    args = parser.parse_args()
    owner, boot = CONTROL.identity()
    plan = CONTROL.load(CONTROL.plain(args.plan, 65536))
    case = plan['case']
    require(plan == {'schema': SCHEMA + '-icon-probe', 'owner': owner, 'host_boot': boot, 'case': case}
            and case['pair'] == 0 and case['arm'] == 'A', 'Wrong qualification plan or owner')
    CONTROL.signal.signal(CONTROL.signal.SIGHUP, interrupted)
    CONTROL.signal.signal(CONTROL.signal.SIGTERM, interrupted)
    batch = case['batch']
    require(re.fullmatch(r'/var/tmp/bilicomics-interactive-bili-shelf-[0-9a-f]{32}', batch), 'Unexpected icon-only batch')
    CONTROL.announce(f'Public icon qualification batch: {batch}')
    IconProbe(Path(batch)).execute(args.recover)


if __name__ == '__main__':
    try:
        main()
    except CONTROL.OperatorError as error:
        sys.exit(str(error))
    except (OSError, ValueError, KeyError, TypeError, KeyboardInterrupt):
        sys.exit('Icon-only observation stopped; check its original batch.')
