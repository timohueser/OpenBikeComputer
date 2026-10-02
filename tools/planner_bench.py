"""Measure complete planner files and HTTP route requests. Reports go to stdout."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import math
from pathlib import Path
import statistics
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen
import zlib


def file_size(path):
    digest = hashlib.sha256()
    compressor = zlib.compressobj(6, zlib.DEFLATED, 31)
    size = compressed = 0
    with path.open('rb') as source:
        while data := source.read(1024 * 1024):
            digest.update(data)
            size += len(data)
            compressed += len(compressor.compress(data))
    compressed += len(compressor.flush())
    return {'bytes': size, 'sha256': digest.hexdigest(), 'gzip6_bytes': compressed}


def audit(root):
    manifest = root / 'release.json'
    release = json.loads(manifest.read_bytes())
    files = {}
    for name, expected in release['files'].items():
        path = root / name
        if not path.resolve().is_relative_to(root.resolve()):
            raise ValueError('Release file outside package')
        actual = file_size(path)
        if any(actual[key] != expected[key] for key in ['bytes', 'sha256']):
            raise ValueError(f'Release checksum mismatch: {name}')
        files[name] = actual
    files['release.json'] = file_size(manifest)
    installed = sum(item['bytes'] for item in files.values())
    transfer = sum(min(item['bytes'], item['gzip6_bytes']) for item in files.values())
    return {'release': files['release.json']['sha256'], 'profiles': release['profiles'], 'files': files,
            'installed_bytes': installed, 'gzip6_or_raw_bytes': transfer,
            'measurement': 'Complete runtime files, including manifests; source mirrors excluded. '
                           'Compressed transfer is a measurement, not an installation format.'}


def summary(samples):
    groups = {}
    for sample in samples:
        key = (sample['profile'], sample.get('cache', 'http'))
        groups.setdefault(key, []).append(sample)
    rows = []
    for (profile, cache), group in sorted(groups.items()):
        elapsed = sorted(item['elapsed_ms'] for item in group)
        successful = [item for item in group if not item.get('error')]
        rows.append({'profile': profile, 'cache': cache, 'samples': len(group),
                     'failures': len(group) - len(successful),
                     'median_ms': statistics.median(elapsed),
                     'p95_ms': elapsed[math.ceil(len(elapsed) * .95) - 1],
                     'worst_ms': elapsed[-1]})
    return rows


def request(url, case):
    start = time.perf_counter()
    row = {'name': case['name'], 'profile': case['request']['profile']}
    try:
        body = json.dumps(case['request']).encode()
        with urlopen(Request(url + '/v1/route', body, {'Content-Type': 'application/json'}), timeout=35) as response:
            value = json.load(response)
        for route in value['routes']:
            route.pop('id', None)
            route.pop('package', None)
        row['result'] = value
    except HTTPError as error:
        row['error'] = json.loads(error.read())
        row['status'] = error.code
    except (OSError, ValueError) as error:
        row['error'] = str(error)
    row['elapsed_ms'] = (time.perf_counter() - start) * 1000
    return row


def server(url, cases, concurrency, iterations):
    with urlopen(url + '/v1/region', timeout=5) as response:
        region = json.load(response)
    start = time.perf_counter()
    with ThreadPoolExecutor(max_workers=concurrency) as executor:
        samples = list(executor.map(lambda case: request(url, case), cases * iterations))
    seconds = time.perf_counter() - start
    return {'region': region, 'concurrency': concurrency, 'seconds': seconds, 'samples': samples,
            'successful_requests_per_second': sum('error' not in row for row in samples) / seconds,
            'summary': summary(samples)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    size = commands.add_parser('audit')
    size.add_argument('release', type=Path)
    report = commands.add_parser('summary')
    report.add_argument('report', type=Path)
    corpus = commands.add_parser('corpus')
    corpus.add_argument('package', type=Path)
    corpus.add_argument('requests', type=Path)
    http = commands.add_parser('server')
    http.add_argument('url')
    http.add_argument('requests', type=Path)
    http.add_argument('--concurrency', type=int, choices=range(1, 9), default=2)
    http.add_argument('--iterations', type=int, default=3)
    args = parser.parse_args()
    if args.command == 'audit':
        output = audit(args.release)
    elif args.command == 'summary':
        output = summary(json.loads(args.report.read_bytes())['samples'])
    elif args.command == 'corpus':
        profiles = json.loads((args.package / 'manifest.json').read_bytes())['metrics']
        output = [{**case, 'request': {**case['request'], 'profile': profile}}
                  for profile in sorted(profiles) for case in json.loads(args.requests.read_bytes())]
    else:
        if args.iterations < 1:
            parser.error('iterations must be positive')
        output = server(args.url.rstrip('/'), json.loads(args.requests.read_bytes()), args.concurrency, args.iterations)
    print(json.dumps(output, sort_keys=True, separators=(',', ':')))


if __name__ == '__main__':
    main()
