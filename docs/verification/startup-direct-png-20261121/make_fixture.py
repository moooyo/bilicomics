"""Build a synthetic, closed catalog from an empty control application's schema, without credentials."""

import argparse
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import sqlite3

from PIL import Image, ImageDraw


WIDTH, HEIGHT = 320, 416
COUNT = 20
FIXTURE_ACCOUNT = 'bili_424242'
SAMPLE_X = (48, 104, 160, 216, 272)
SAMPLE_Y = 208
SWATCH = (45, 170, 114, 112)
TABLES = {
    'comics', 'comic_details', 'catalog_members', 'image_indices', 'reading_anchors', 'preferences',
    'downloads', 'page_receipts', 'page_commit_journal', 'page_identity_history', 'download_event_outbox',
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def application_key(value):
    return hashlib.sha256(value.encode('utf-8')).hexdigest()


def require_closed(database):
    require(database.is_file() and not database.is_symlink(), 'Expected a regular SQLite database')
    for suffix in ('-wal', '-shm', '-journal'):
        require(not os.path.lexists(str(database) + suffix), 'Checkpoint and close the database before copying it')


def open_closed(database):
    require_closed(database)
    return sqlite3.connect(database.resolve().as_uri() + '?mode=ro&immutable=1', uri=True)


def control_schema(directory):
    require(not directory.is_symlink() and not (directory / 'demo').exists(), 'Expected an ordinary control data directory')
    require(not os.path.lexists(directory / 'account/private/session.json'), 'Do not use a directory containing a session')
    with closing(open_closed(directory / 'state.sqlite3')) as connection:
        version = connection.execute('PRAGMA user_version').fetchone()[0]
        require(version == 4, 'Expected the schema created by the qualified control')
        objects = connection.execute("SELECT type,name,sql FROM sqlite_master WHERE sql IS NOT NULL AND substr(name,1,7) != 'sqlite_' ORDER BY type DESC,name").fetchall()
        require(all(kind in ('table', 'index') for kind, _, _ in objects), 'Unexpected object in the control schema')
        require({name for kind, name, _ in objects if kind == 'table'} == TABLES, 'Unexpected control tables')
        for name in sorted(TABLES - {'preferences'}):
            require(connection.execute(f'SELECT 1 FROM "{name}" LIMIT 1').fetchone() is None, 'The control must start with an empty catalog and no account data')
        require(connection.execute("SELECT 1 FROM preferences WHERE account_key NOT IN ('guest','device') LIMIT 1").fetchone() is None, 'Unexpected account in the control database')
    return [definition for _, _, definition in objects]


def make_cover(path, number):
    image = Image.new('RGBA', (WIDTH, HEIGHT))
    brush = ImageDraw.Draw(image)
    for top in range(0, HEIGHT, 5):
        for left in range(0, WIDTH, 5):
            grain = ((left // 5) * 13 + (top // 5) * 19 + (left // 13) * (top // 17) + number * 23) % 87
            band = ((left + (2 + number % 5) * top) // 34 + number) % 3
            brush.rectangle((left, top, left + 4, top + 4), fill=(24 + 58 * band + grain, 50 + grain,
                            215 - 45 * band - grain, (255, 164, 208)[band]))
    samples = []
    for bit, center in zip(range(4, -1, -1), SAMPLE_X):
        level = 224 if number & (1 << bit) else 32
        brush.rectangle((center - 21, SAMPLE_Y - 36, center + 21, SAMPLE_Y + 36), fill=(level, level, level, 255))
        samples.append({'xy': [center, SAMPLE_Y], 'gray': level})
    brush.rectangle((137, 82, 183, 118), fill=SWATCH)
    with path.open('xb') as output:
        image.save(output, format='PNG', compress_level=6)
    with path.open('rb') as encoded:
        header = encoded.read(29)
        require(header[:8] == b'\x89PNG\r\n\x1a\n' and header[12:16] == b'IHDR'
                and header[24:29] == bytes((8, 6, 0, 0, 0)), 'Expected a non-interlaced RGBA8 PNG')
    with Image.open(path) as decoded:
        require(decoded.n_frames == 1 and decoded.mode == 'RGBA' and decoded.size == (WIDTH, HEIGHT), 'Unexpected generated image')
        require(decoded.getpixel((160, 100)) == SWATCH, 'The translucent swatch changed')
        for sample in samples:
            level = sample['gray']
            require(decoded.getpixel(tuple(sample['xy'])) == (level, level, level, 255), 'The visible identifier changed')
    return {'samples': samples, 'translucent': {'xy': [160, 100], 'rgba': list(SWATCH)}}


def profile(count, without_identity):
    per_image = WIDTH * HEIGHT * 4
    per_batch = ((4 << 20) + per_image - 1) // per_image
    return {'first_page_ids': [str(710000 + index) for index in range(1, count + 1)],
            'prepared_covers': count, 'visible_covers_with_fixture_identity': count, 'visible_covers_without_identity': without_identity,
            'pages': (COUNT + count - 1) // count,
            'prepared_rgba_bytes': count * per_image,
            'attachment_batches': [min(per_batch, count - start) for start in range(0, count, per_batch)]}


def generate(control, seed, receipt):
    definitions = control_schema(control)
    require(receipt.resolve() != seed.resolve() and seed.resolve() not in receipt.resolve().parents,
            'Keep the receipt outside the database seed')
    require(not os.path.lexists(receipt), 'The receipt path already exists')
    seed.mkdir(mode=0o700)
    cover_dir = seed / 'covers'
    cover_dir.mkdir(mode=0o700)
    store_scope = 'store:' + application_key('all')
    covers = []
    database = seed / 'state.sqlite3'
    with closing(sqlite3.connect(database)) as connection:
        connection.execute('PRAGMA journal_mode=WAL')
        connection.execute('PRAGMA synchronous=FULL')
        with connection:
            connection.execute('BEGIN')
            for definition in definitions:
                connection.execute(definition)
            connection.execute('PRAGMA user_version=4')
            for position in range(COUNT):
                number = position + 1
                comic_id = str(710000 + number)
                title = f'Archive Study {number:02d}'
                url = f'https://covers.fixture.invalid/studies/{comic_id}.png'
                name = application_key(url) + '.png'
                marker = make_cover(cover_dir / name, number)
                covers.append({'position': position, 'id': comic_id, 'title': title, 'file': f'covers/{name}',
                               'width': WIDTH, 'height': HEIGHT, 'encoded_bytes': (cover_dir / name).stat().st_size, 'marker': marker})
                updated = 1700000000 - position
                comic = {'id': comic_id, 'title': title, 'author': 'Fixture Studio', 'cover_url': url,
                         'favorite': True, 'chapter_count': 3, 'updated_at': updated, 'last_read_at': 0}
                connection.execute('INSERT INTO comics(account_key,comic_id,payload,updated_at,last_read_at) VALUES(?,?,?,?,0)',
                                   (FIXTURE_ACCOUNT, comic_id, json.dumps(comic), updated))
                for scope in ('shelf', store_scope):
                    connection.execute('INSERT INTO catalog_members(account_key,scope,comic_id,position) VALUES(?,?,?,?)',
                                       (FIXTURE_ACCOUNT, scope, comic_id, position))
            preferences = {'preferences': {'onboarded': True, 'launch': 'shelf', 'reader_guidance_seen': True},
                           'catalog_shelf_initialized': True, 'catalog_view': {'view': 'grid', 'filter': 'all',
                               'category': 'all', 'shelf_page': 1, 'store_page': 1},
                           'catalog_cache_recency': {'scopes': [store_scope], 'comics': [item['id'] for item in covers], 'details': []}}
            connection.executemany('INSERT INTO preferences(account_key,key,payload) VALUES(?,?,?)',
                                   [(FIXTURE_ACCOUNT, key, json.dumps(value)) for key, value in preferences.items()])
            connection.execute("INSERT INTO preferences(account_key,key,payload) VALUES('device','last_account',?)", (json.dumps(FIXTURE_ACCOUNT),))
        require(connection.execute('PRAGMA wal_checkpoint(TRUNCATE)').fetchone() == (0, 0, 0), 'Could not checkpoint the seed')
    with closing(open_closed(database)) as connection:
        for scope in ('shelf', store_scope):
            rows = connection.execute("SELECT c.comic_id,c.payload FROM catalog_members m JOIN comics c "
                                      "ON c.account_key=m.account_key AND c.comic_id=m.comic_id "
                                      'WHERE m.account_key=? AND m.scope=? ORDER BY m.position', (FIXTURE_ACCOUNT, scope)).fetchall()
            require([comic for comic, _ in rows] == [cover['id'] for cover in covers], 'Closed catalog membership differs')
            for (_, payload), item in zip(rows, covers):
                require('covers/' + application_key(json.loads(payload)['cover_url']) + '.png' == item['file'], 'The cache key does not resolve')
    result = {'source': 'Empty ordinary control schema; no source rows or application files were copied.',
              'scope': 'Synthetic offline cache only. The identical seed remains signed out unless the separate private mock identity setup succeeds.',
              'account_key': FIXTURE_ACCOUNT, 'onboarded': True, 'session_present': False, 'resume_present': False,
              'schema_version': 4, 'cover_encoding': 'static straight RGBA8 PNG', 'covers': covers,
              'profiles': {'scribe': {'store': profile(15, 15), 'shelf': profile(10, 0)}}}
    with receipt.open('x', encoding='utf-8') as output:
        json.dump(result, output, indent=2)
        output.write('\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('control_data', type=Path, help='Fresh empty ordinary control data directory, after managed application exit')
    parser.add_argument('seed', type=Path, help='New closed data directory for independent per-run copies')
    parser.add_argument('receipt', type=Path, help='New JSON file outside the seed')
    arguments = parser.parse_args()
    os.umask(0o077)
    generate(arguments.control_data, arguments.seed, arguments.receipt)
