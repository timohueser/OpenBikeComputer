"""Extract a bounding box and its search dependencies from a complete package."""
import argparse
import hashlib
import json
import math
import sqlite3
from pathlib import Path

from storage import create, finish


def extract(source, destination, bounds):
    if len(bounds) != 4 or not (-180 <= bounds[0] < bounds[2] <= 180 and -85 <= bounds[1] < bounds[3] <= 85):
        raise ValueError('Use west,south,east,north bounds within the supported map area.')
    uri = source.resolve().as_uri() + '?mode=ro'
    with sqlite3.connect(uri, uri=True) as original:
        meta = {k: json.loads(v) for k, v in original.execute('SELECT * FROM metadata')}
    if meta.get('schema') != 3:
        raise ValueError('Rebuild the source package with schema 3.')
    coverage = meta.get('bounds')
    if not isinstance(coverage, list) or len(coverage) != 4 or not all(isinstance(v, (int, float)) and math.isfinite(v) for v in coverage):
        raise ValueError('The source has no valid coverage bounds. Rebuild it with --bounds.')
    if not (coverage[0] <= bounds[0] < bounds[2] <= coverage[2]
            and coverage[1] <= bounds[1] < bounds[3] <= coverage[3]):
        raise ValueError('The source package does not cover these bounds.')
    with source.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    db = create(destination)
    try:
        db.execute('ATTACH DATABASE ? AS original', (uri,))
        db.execute('CREATE TEMP TABLE selected(id INTEGER PRIMARY KEY)')
        db.execute('''INSERT INTO selected SELECT id FROM original.places
            WHERE (lon>=? AND lat>=? AND lon<=? AND lat<=?)
               OR (east>=? AND north>=? AND west<=? AND south<=?)''', (*bounds, *bounds))
        db.execute('''INSERT INTO addresses(rowid,street_id,house,lon,lat,source)
            SELECT rowid,street_id,house,lon,lat,source FROM original.addresses
            WHERE lon>=? AND lat>=? AND lon<=? AND lat<=?''', bounds)
        db.execute('INSERT OR IGNORE INTO selected SELECT DISTINCT street_id FROM addresses')
        db.execute('INSERT INTO place_records SELECT * FROM original.place_records WHERE id IN selected ORDER BY id')
        db.execute('''INSERT INTO place_contexts SELECT * FROM original.place_contexts
            WHERE id IN (SELECT context_id FROM place_records) ORDER BY id''')
        db.commit()
        db.execute('DETACH DATABASE original')
        finish(db, destination, {**meta, 'bounds': bounds, 'source_search_sha256': digest})
    except BaseException:
        db.close()
        raise


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('destination', type=Path)
    parser.add_argument('--bounds', required=True, help='West,south,east,north')
    args = parser.parse_args()
    extract(args.source, args.destination, list(map(float, args.bounds.split(','))))
