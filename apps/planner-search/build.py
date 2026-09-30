"""Build searchable regional SQLite files from a Photon/Nominatim JSON dump."""
import argparse
import hashlib
import io
import sqlite3
import time
import re
from collections import Counter
from pathlib import Path

import orjson
import zstandard
from shapely.geometry import mapping, shape
from index import norm, index_search

ROOT = Path(__file__).parent
SCHEMA = """
PRAGMA journal_mode=OFF;
PRAGMA synchronous=OFF;
PRAGMA cache_size=-32768;
PRAGMA temp_store=FILE;
CREATE TABLE places(id INTEGER PRIMARY KEY, source TEXT, name TEXT, aliases TEXT,
 kind TEXT, lon REAL, lat REAL, city TEXT, postcode TEXT, importance REAL,
 west REAL, south REAL, east REAL, north REAL, region TEXT, context TEXT, cuisine TEXT, opening_hours TEXT);
CREATE TABLE addresses(street_id INTEGER, house TEXT, lon REAL, lat REAL, source TEXT);
CREATE TABLE metadata(key TEXT PRIMARY KEY, value TEXT);
"""


def values(d, keys):
    out = []
    for k in keys:
        v = d.get(k, '')
        out.extend(v if isinstance(v, list) else [v])
    return list(dict.fromkeys(str(v) for v in out if v))


def names(d):
    return values(d, ['name', 'name:de', 'name:en', 'name:fr', 'name:it', 'name:es',
                      'name:nl', 'alt_name', 'loc_name', 'short_name', 'official_name', 'int_name'])


def category(p):
    k, v = p['osm_key'], p['osm_value']
    if k == 'mountain_pass':
        return 'pass'
    if v == 'yes':
        return k
    if p.get('address_type') in ('city', 'town', 'village', 'hamlet', 'suburb', 'district', 'state', 'country'):
        return p['address_type']
    if k == 'boundary':
        return 'locality'
    if p.get('address_type') == 'street':
        return 'street'
    return {'peak': 'summit', 'saddle': 'pass', 'camp_site': 'campsite',
            'alpine_hut': 'hut', 'wilderness_hut': 'hut', 'bicycle': 'bike_shop',
            'bicycle_repair_station': 'repair_station', 'station': 'train_station',
            'water': 'lake', 'doctors': 'doctor', 'charging_station': 'charging',
            'ferry_terminal': 'ferry', 'public_bath': 'shower'}.get(v, v)


class Writer:
    def __init__(self, name, output):
        self.path = output / f'{name}.sqlite'
        if self.path.exists():
            raise SystemExit(f'{self.path} exists; choose a fresh output directory or remove this generated file.')
        self.db = sqlite3.connect(self.path)
        self.db.executescript(SCHEMA)
        self.streets = {}
        self.counts = Counter()
        self.next_id = 0

    def place(self, source, name, aliases, kind, lon, lat, city, postcode, importance, bbox, region, context, cuisine='', opening_hours=''):
        self.next_id += 1
        self.db.execute('INSERT INTO places VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
                        (self.next_id, source, name, aliases, kind, lon, lat, city, postcode,
                         importance, *bbox, region, context, cuisine, opening_hours))
        self.counts[kind] += 1
        return self.next_id

    def add(self, p):
        # Photon derives postcode centroids from addresses without an OSM identity.
        if p['osm_key'] == 'place' and p['osm_value'] == 'postcode' and not p.get('object_type'):
            return
        a = p.get('address', {})
        ns = names(p.get('name', {}))
        lon, lat = p['centroid']
        region = a.get('state', '')
        city = (values(a, ['city', 'town', 'village', 'county']) or [''])[0]
        postcode = p.get('postcode', '')
        country = 'Deutschland Germany' if p.get('country_code') == 'de' else ' '.join(values(a, ['country', 'country:en']))
        context = ' '.join(values(a, ['city', 'city:de', 'city:en', 'district', 'locality',
                                     'county', 'state', 'street']) + [postcode, country])
        source = f"{p['object_type'].lower()}{p['object_id']}"
        bbox = p.get('bbox', [lon, lat, lon, lat])
        bbox = [min(bbox[0], bbox[2]), min(bbox[1], bbox[3]), max(bbox[0], bbox[2]), max(bbox[1], bbox[3])]
        kind = category(p)
        street = a.get('street', '')
        house = p.get('housenumber', '')
        if kind == 'street' and ns:
            street = ns[0]
        if street and (house or kind == 'street'):
            key = (street, city, postcode, region)
            sid = self.streets.get(key)
            if sid is None:
                stable = 's' + hashlib.sha1('|'.join(key).encode()).hexdigest()[:20]
                aliases = ';'.join(ns) if kind == 'street' else street
                sid = self.place(stable, street, aliases, 'street', lon, lat, city, postcode,
                                 0.05, bbox, region, context)
                self.streets[key] = sid
            if house:
                self.db.execute('INSERT INTO addresses VALUES (?,?,?,?,?)', (sid, norm(house), lon, lat, source))
                self.counts['address'] += 1
        if kind == 'street':
            return
        # Unnamed service features remain category-searchable.
        services = {'bakery', 'supermarket', 'convenience', 'campsite', 'hotel', 'hostel', 'hut',
                    'guest_house', 'drinking_water', 'water_point', 'spring', 'pharmacy',
                    'bike_shop', 'repair_station', 'restaurant', 'cafe', 'shelter', 'toilets',
                    'train_station', 'museum', 'viewpoint', 'pass', 'summit', 'fast_food',
                    'water_tap', 'fountain', 'motel', 'butcher', 'marketplace', 'fuel',
                    'bar', 'ice_cream', 'hospital', 'doctor', 'clinic', 'charging', 'shower',
                    'laundry', 'atm', 'bus_stop', 'ferry', 'lake', 'beach', 'swimming_pool',
                    'castle', 'church', 'monastery', 'ruins', 'waterfall', 'tower', 'bridge'}
        if not ns and kind not in services:
            return
        if p['osm_key'] == 'building' and house and not ns:
            return
        self.place(source, ns[0] if ns else kind.replace('_', ' '), ';'.join(ns), kind,
                   lon, lat, city, postcode, p.get('importance', 0) or 0, bbox, region, context,
                   p.get('extra', {}).get('cuisine', ''), p.get('extra', {}).get('opening_hours', ''))

    def finish(self, meta):
        self.db.commit()
        print(f'Indexing {self.path.name}: {self.next_id:,} places, {self.counts["address"]:,} addresses', flush=True)
        self.db.executescript('''
          CREATE INDEX address_lookup ON addresses(street_id,house);
          CREATE VIRTUAL TABLE address_spatial USING rtree(id,west,east,south,north);
          INSERT INTO address_spatial SELECT rowid,lon,lon,lat,lat FROM addresses;
          CREATE INDEX category_lookup ON places(kind,lon,lat);
          CREATE INDEX source_lookup ON places(source);
          CREATE VIRTUAL TABLE spatial USING rtree(id,west,east,south,north);
          INSERT INTO spatial SELECT id,lon,lon,lat,lat FROM places;
        ''')
        index_search(self.db)
        meta = {**meta, 'counts': dict(self.counts)}
        self.db.executemany('INSERT INTO metadata VALUES (?,?)', ((k, orjson.dumps(v).decode()) for k, v in meta.items()))
        self.db.commit()
        self.db.close()
        print(f'{self.path.name}: {self.path.stat().st_size / 1e6:.1f} MB', flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('dump', type=Path)
    ap.add_argument('--limit', type=int, default=0)
    ap.add_argument('--output', type=Path, default=ROOT / 'data')
    ap.add_argument('--region', default='all')
    ap.add_argument('--bounds', help='West,south,east,north for one regional package')
    ap.add_argument('--countries', default='de', help='Comma-separated country codes')
    ap.add_argument('--osm-sha256', help='Identity of the OSM input used by Nominatim')
    args = ap.parse_args()
    if not re.fullmatch(r'[a-z][a-z0-9-]{0,63}', args.region):
        ap.error('Invalid region ID')
    bounds = list(map(float, args.bounds.split(','))) if args.bounds else None
    if bounds is None and args.region not in ('all', 'germany', 'baden-wuerttemberg'):
        ap.error('--bounds is required for a custom region')
    if bounds and (len(bounds) != 4 or not -180 <= bounds[0] < bounds[2] <= 180 or not -85 <= bounds[1] < bounds[3] <= 85 or args.region == 'all'):
        ap.error('Use valid bounds with one region')
    countries = args.countries.lower().split(',')
    if args.osm_sha256 and not re.fullmatch(r'[a-f0-9]{64}', args.osm_sha256):
        ap.error('Invalid OSM digest')
    args.output.mkdir(parents=True, exist_ok=True)
    regions = ['germany', 'baden-wuerttemberg'] if args.region == 'all' else [args.region]
    writers = {name: Writer(name, args.output) for name in regions}
    start = time.monotonic()
    n = 0
    outlines = []
    meta = {'schema': 2, 'source': args.dump.name, 'attribution': '© OpenStreetMap contributors, ODbL 1.0; prepared by Nominatim / Photon'}
    if args.osm_sha256:
        meta.update(osm_sha256=args.osm_sha256, bounds=bounds, countries=countries)
    with args.dump.open('rb') as raw, zstandard.ZstdDecompressor().stream_reader(raw) as stream:
        for line in io.BufferedReader(stream):
            obj = orjson.loads(line)
            if obj['type'] == 'NominatimDumpFile':
                meta['timestamp'] = obj['content']['data_timestamp']
            if obj['type'] != 'Place':
                continue
            for p in obj['content']:
                if p.get('country_code') not in countries or not p.get('centroid'):
                    continue
                if bounds:
                    lon, lat = p['centroid']
                    if not (bounds[0] <= lon <= bounds[2] and bounds[1] <= lat <= bounds[3]):
                        continue
                    writers[args.region].add(p)
                elif 'germany' in writers:
                    writers['germany'].add(p)
                state = p.get('address', {}).get('state', '')
                name = p.get('name', {}).get('name', '')
                if not bounds and 'baden-wuerttemberg' in writers and (state == 'Baden-Württemberg' or name == 'Baden-Württemberg'):
                    writers['baden-wuerttemberg'].add(p)
                if p.get('address_type') == 'state' and p.get('geometry'):
                    geom = mapping(shape(p['geometry']).simplify(.003, preserve_topology=True))
                    outlines.append({'type': 'Feature', 'properties': {'name': name}, 'geometry': geom})
                n += 1
                if n % 100000 == 0:
                    for w in writers.values():
                        w.db.commit()
                    print(f'{n:,} records, {time.monotonic()-start:.0f}s', flush=True)
                if args.limit and n >= args.limit:
                    break
            if args.limit and n >= args.limit:
                break
    (args.output / 'regions.geojson').write_bytes(orjson.dumps({'type':'FeatureCollection','features':outlines}))
    for w in writers.values():
        w.finish(meta)
    print(f'Complete: {n:,} records, {time.monotonic()-start:.0f}s', flush=True)


if __name__ == '__main__':
    main()
