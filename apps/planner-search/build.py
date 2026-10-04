"""Build searchable regional SQLite files from a Photon/Nominatim JSON dump."""
import argparse
import io
import time
import re
from pathlib import Path

from writer import Writer

ROOT = Path(__file__).parent


def main():
    import orjson
    import zstandard
    from shapely.geometry import mapping, shape

    ap = argparse.ArgumentParser()
    ap.add_argument('dump', type=Path)
    ap.add_argument('--component', choices=['all', 'pois', 'addresses'], default='all')
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
    writers = {name: Writer(name, args.output, args.component) for name in regions}
    start = time.monotonic()
    n = 0
    outlines = []
    meta = {'schema': 4, 'source': args.dump.name, 'attribution': '© OpenStreetMap contributors, ODbL 1.0; prepared by Nominatim / Photon'}
    if bounds:
        meta.update(bounds=bounds, countries=countries)
    if args.osm_sha256:
        meta['osm_sha256'] = args.osm_sha256
    with args.dump.open('rb') as raw, zstandard.ZstdDecompressor().stream_reader(raw) as stream:
        for line in io.BufferedReader(stream):
            obj = orjson.loads(line)
            if obj['type'] == 'NominatimDumpFile':
                meta['timestamp'] = obj['content']['data_timestamp']
                generator = obj['content'].get('generator', 'photon')
                source_hash = obj['content'].get('osm_sha256')
                if source_hash and args.osm_sha256 and source_hash != args.osm_sha256:
                    ap.error('Source and requested OSM snapshot differ')
                if obj['content'].get('scope') == 'addresses' and args.component != 'addresses':
                    ap.error('This source contains addresses only; use --component addresses')
                if generator != 'photon':
                    meta['source_generator'] = generator
                    meta['attribution'] = f'© OpenStreetMap contributors, ODbL 1.0; prepared by {generator}'
            if obj['type'] != 'Place':
                continue
            for p in obj['content']:
                if p.get('country_code') not in countries or not p.get('centroid'):
                    continue
                if bounds:
                    lon, lat = p['centroid']
                    extent = p.get('bbox', [lon, lat, lon, lat])
                    if not ((bounds[0] <= lon <= bounds[2] and bounds[1] <= lat <= bounds[3])
                            or (max(extent[0], extent[2]) >= bounds[0] and max(extent[1], extent[3]) >= bounds[1]
                                and min(extent[0], extent[2]) <= bounds[2] and min(extent[1], extent[3]) <= bounds[3])):
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
