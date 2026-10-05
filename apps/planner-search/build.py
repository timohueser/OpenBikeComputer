"""Build searchable regional SQLite files from a enriched OSM search dump."""
import argparse
import io
import sys
import time
import re
from pathlib import Path
from zoneinfo import ZoneInfo

from writer import Writer

ROOT = Path(__file__).parent


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('dump', type=Path)
    ap.add_argument('--component', choices=['all', 'pois', 'addresses'], default='all')
    ap.add_argument('--limit', type=int, default=0)
    ap.add_argument('--output', type=Path, default=ROOT / 'data')
    ap.add_argument('--region', default='all')
    ap.add_argument('--bounds', help='West,south,east,north for one regional package')
    ap.add_argument('--countries', default='de', help='Comma-separated country codes')
    ap.add_argument('--osm-sha256', help='Identity of the OSM input used by the search baker')
    ap.add_argument('--time-zone', required=True, type=ZoneInfo, help='IANA time zone of the region calendar')
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
    # A step gets its credit in its options, so only this command line reads the registry.
    sys.path.insert(0, str(ROOT.parents[1] / "tools"))
    from data_registry import attribution

    try:
        build(args.dump, args.dump.name, args.output, args.component, args.region, bounds, countries,
              args.time_zone, attribution("osm-planet"), args.osm_sha256, args.limit)
    except ValueError as error:
        ap.error(str(error))


def build(dump, source, output, component, region, bounds, countries, time_zone, credit, osm_sha256=None, limit=0):
    """Write the search databases of `component` from the dump `dump`, whose name in their metadata
    is `source`. `credit` is the attribution of the OSM data."""
    import orjson
    import zstandard
    from shapely.geometry import mapping, shape

    output.mkdir(parents=True, exist_ok=True)
    regions = ['germany', 'baden-wuerttemberg'] if region == 'all' else [region]
    writers = {name: Writer(name, output, component) for name in regions}
    start = time.monotonic()
    n = 0
    outlines = []
    meta = {'source': source, 'time_zone': time_zone.key, 'attribution': credit}
    if bounds:
        meta.update(bounds=bounds, countries=countries)
    if osm_sha256:
        meta['osm_sha256'] = osm_sha256
    with dump.open('rb') as raw, zstandard.ZstdDecompressor().stream_reader(raw) as stream:
        for line in io.BufferedReader(stream):
            obj = orjson.loads(line)
            if obj['type'] == 'NominatimDumpFile':
                generator = obj['content'].get('generator', 'photon')
                source_hash = obj['content'].get('osm_sha256')
                if source_hash and osm_sha256 and source_hash != osm_sha256:
                    raise ValueError('Source and requested OSM snapshot differ')
                if source_hash:
                    if not re.fullmatch(r'[a-f0-9]{64}', source_hash):
                        raise ValueError('Invalid source OSM digest')
                    meta['osm_sha256'] = source_hash
                # After the digest: the order of the metadata rows is the same with or without `osm_sha256`.
                meta['timestamp'] = obj['content']['data_timestamp']
                if obj['content'].get('scope') == 'addresses' and component != 'addresses':
                    raise ValueError('This source contains addresses only; use --component addresses')
                if generator != 'photon':
                    meta['source_generator'] = generator
                    meta['attribution'] = f'{credit}; prepared by {generator}'
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
                    writers[region].add(p)
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
                if limit and n >= limit:
                    break
            if limit and n >= limit:
                break
    (output / 'regions.geojson').write_bytes(orjson.dumps({'type':'FeatureCollection','features':outlines}))
    for w in writers.values():
        w.finish(meta)
    print(f'Complete: {n:,} records, {time.monotonic()-start:.0f}s', flush=True)


def step():
    """The `obc data` step `planner/search/<component>`: `<component>/<region>.sqlite` from the
    records of `planner/search/records`."""
    sys.path.insert(0, str(ROOT.parents[1]))
    from tools import step_request

    request = step_request.read()
    options = request['options']
    component = options['component']
    name = f'{component}.jsonl.zst'
    dump = Path(request['layers']['planner/search/records'][name])
    output = Path(request['output']) / component
    build(dump, name, output, component, options['region'], options['bounds'],
          [country.lower() for country in options['countries']], ZoneInfo(options['time_zone']),
          options['attribution'])
    (output / 'regions.geojson').unlink()


if __name__ == '__main__':
    step() if sys.argv[1:] == ['--step'] else main()
