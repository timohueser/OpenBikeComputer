"""Prepare rail-only GTFS and measure a local MOTIS timetable service."""

import argparse
from collections import Counter
import csv
from datetime import datetime
import hashlib
import io
import json
import math
from pathlib import Path
import statistics
import time
from urllib.parse import urlencode
from urllib.request import urlopen
import zipfile


def fingerprint(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def rows(archive, name):
    with archive.open(name) as source:
        yield from csv.DictReader(io.TextIOWrapper(source, encoding='utf-8-sig', newline=''))


def prepare(source, target):
    if source.resolve() == target.resolve():
        raise ValueError('Input and output must differ')
    counts = {}
    bikes = Counter()
    start = time.perf_counter()
    with zipfile.ZipFile(source) as archive:
        rail_routes = {row['route_id'] for row in rows(archive, 'routes.txt')
                       if int(row['route_type']) == 2 or 100 <= int(row['route_type']) <= 117}
    if not rail_routes:
        raise ValueError('No rail routes in feed')
    with zipfile.ZipFile(source) as src, zipfile.ZipFile(target, 'x', zipfile.ZIP_DEFLATED) as dst:
        def copy(name, keep, visit=lambda row: None, omit=()):
            if name not in src.namelist():
                return
            with src.open(name) as raw, dst.open(name, 'w') as out:
                reader = csv.DictReader(io.TextIOWrapper(raw, encoding='utf-8-sig', newline=''))
                fields = [key for key in reader.fieldnames if key not in omit]
                with io.TextIOWrapper(out, encoding='utf-8', newline='') as text:
                    writer = csv.DictWriter(text, fields, extrasaction='ignore', lineterminator='\n')
                    writer.writeheader()
                    total = retained = 0
                    for row in reader:
                        total += 1
                        if keep(row):
                            visit(row)
                            writer.writerow(row)
                            retained += 1
            counts[name] = {'source_rows': total, 'rail_rows': retained}

        agencies = set()
        copy('routes.txt', lambda row: row['route_id'] in rail_routes,
             lambda row: agencies.add(row.get('agency_id', '')))
        trips, services, stops = set(), set(), set()

        def trip(row):
            trips.add(row['trip_id'])
            services.add(row['service_id'])
            bikes[row.get('bikes_allowed') or '0'] += 1

        copy('trips.txt', lambda row: row['route_id'] in rail_routes, trip, omit=('shape_id',))

        def stop_time(row):
            if not row.get('stop_id'):
                raise ValueError('Rail trip has a flexible stop; this probe needs fixed stops')
            stops.add(row['stop_id'])

        copy('stop_times.txt', lambda row: row['trip_id'] in trips, stop_time)
        parents = {row['stop_id']: row.get('parent_station', '') for row in rows(src, 'stops.txt')}
        pending = list(stops)
        while pending:
            parent = parents[pending.pop()]
            if parent and parent not in stops:
                stops.add(parent)
                pending.append(parent)
        copy('stops.txt', lambda row: row['stop_id'] in stops, omit=('level_id',))
        if '' in agencies:
            agencies.update(row.get('agency_id', '') for row in rows(src, 'agency.txt'))
        copy('agency.txt', lambda row: '' in agencies or row.get('agency_id', '') in agencies)
        for name in ('calendar.txt', 'calendar_dates.txt'):
            copy(name, lambda row: row['service_id'] in services)
        copy('frequencies.txt', lambda row: row['trip_id'] in trips)

        def references(row):
            return all(not row.get(key) or row[key] in ids for key, ids in (
                ('from_stop_id', stops), ('to_stop_id', stops),
                ('from_route_id', rail_routes), ('to_route_id', rail_routes),
                ('from_trip_id', trips), ('to_trip_id', trips),
                ('agency_id', agencies), ('route_id', rail_routes), ('trip_id', trips)))

        copy('transfers.txt', references)
        copy('attributions.txt', references)
        copy('feed_info.txt', lambda row: True)
        feed_info = list(rows(src, 'feed_info.txt')) if 'feed_info.txt' in src.namelist() else []
        operators = [{key: row.get(key, '') for key in (
            'agency_id', 'agency_name', 'agency_url', 'agency_timezone')}
            for row in rows(src, 'agency.txt')
            if '' in agencies or row.get('agency_id', '') in agencies]
        omitted = sorted(set(src.namelist()) - set(dst.namelist()))
    return {'source': str(source), 'source_sha256': fingerprint(source),
            'source_bytes': source.stat().st_size, 'rail': str(target),
            'rail_sha256': fingerprint(target), 'rail_bytes': target.stat().st_size,
            'tables': counts, 'bikes_allowed_trip_rows': dict(bikes),
            'feed_info': feed_info, 'operators': operators,
            'omitted_tables': omitted, 'prepare_seconds': time.perf_counter() - start}


def get(url, endpoint, params):
    with urlopen(url.rstrip('/') + '/api/v6/' + endpoint + '?' + urlencode(params), timeout=60) as response:
        return json.load(response)


def departure_window(url, case):
    """Measure the union of one-to-all queries at origin departure minutes."""
    start, end = (datetime.fromisoformat(case[key]) for key in ('start', 'end'))
    if start.tzinfo is None or end.tzinfo is None or end <= start:
        raise ValueError('Use an increasing window with explicit UTC offsets')
    if any(t.second or t.microsecond for t in (start, end)):
        raise ValueError('Window bounds must align to minutes')
    begin = time.perf_counter()
    params = {'stopId': case['station'], 'time': case['start'], 'n': 1000,
              'mode': 'RAIL', 'realtimeMode': 'OFF'}
    departures, cursors = set(), set()
    while True:
        page = get(url, 'stoptimes', params)
        events = [datetime.fromisoformat(row['place']['departure']) for row in page['stopTimes']]
        departures.update(t.replace(second=0, microsecond=0) for t in events if start <= t < end)
        if not events or max(events) >= end:
            break
        cursor = page.get('nextPageCursor')
        if not cursor or cursor in cursors:
            raise ValueError('Departure paging did not cover the window')
        cursors.add(cursor)
        params['pageCursor'] = cursor
    reachable = {}
    for departure in sorted(departures):
        answer = get(url, 'one-to-all', {
            'one': case['station'], 'time': departure.isoformat(),
            'maxTravelTime': case['max_minutes'], 'maxTransfers': case['changes'],
            'transitModes': 'RAIL', 'useRoutedTransfers': 'false'})
        for row in answer['all']:
            stop = row['place']['stopId']
            if stop not in reachable or row['duration'] < reachable[stop]['duration']:
                reachable[stop] = {'duration': row['duration'], 'boardings': row['k'],
                                   'query_start': departure.isoformat()}
    return {'case': case, 'method': 'origin departure minute snapshots',
            'departure_minutes': len(departures), 'reachable_stop_records': len(reachable),
            'elapsed_seconds': time.perf_counter() - begin, 'reachable': reachable}


def benchmark(url, cases, runs):
    if runs < 2:
        raise ValueError('Use at least two runs')
    result = []
    for case in cases:
        params = {'one': case['station'], 'time': case['start'],
                  'maxTravelTime': case['max_minutes'], 'maxTransfers': case['changes'],
                  'transitModes': 'RAIL', 'useRoutedTransfers': 'false'}
        samples = []
        for _ in range(runs):
            start = time.perf_counter()
            answer = get(url, 'one-to-all', params)
            samples.append((time.perf_counter() - start) * 1000)
        warm = sorted(samples[1:])
        row = {'case': case, 'runs': runs, 'first_ms': samples[0],
               'warm_median_ms': statistics.median(warm),
               'warm_p95_ms': warm[math.ceil(len(warm) * .95) - 1],
               'reachable_stop_records': len(answer['all']),
               'response_bytes': len(json.dumps(answer).encode())}
        if 'to' in case:
            journey_params = {key: value for key, value in params.items() if key != 'one'}
            journey_params.update(fromPlace=case['station'], toPlace=case['to'],
                                  timetableView='false', directModes='')
            row['journey'] = get(url, 'plan', journey_params)
        result.append(row)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    prepare_parser = sub.add_parser('prepare')
    prepare_parser.add_argument('source', type=Path)
    prepare_parser.add_argument('target', type=Path)
    bench_parser = sub.add_parser('bench')
    bench_parser.add_argument('--url', default='http://127.0.0.1:8080')
    bench_parser.add_argument('--cases', type=Path, required=True)
    bench_parser.add_argument('--runs', type=int, default=21)
    window_parser = sub.add_parser('window')
    window_parser.add_argument('--url', default='http://127.0.0.1:8080')
    window_parser.add_argument('--case', type=Path, required=True)
    args = parser.parse_args()
    if args.command == 'prepare':
        result = prepare(args.source, args.target)
    elif args.command == 'bench':
        result = benchmark(args.url, json.loads(args.cases.read_text()), args.runs)
    else:
        result = departure_window(args.url, json.loads(args.case.read_text()))
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
