"""Verify stored planner grid artifacts without preparing data or runtimes."""

import argparse
import gzip
import json
from pathlib import Path

from . import planner_grid_index as index, planner_offline as offline, planner_runtime as runtime
from .planner_grid_search import search_metadata
from .planner_map_archive import pixels


def archive(path, kind):
    from pmtiles.reader import Reader, MmapSource, all_tiles
    from pmtiles.tile import Compression, TileType

    with path.open('rb') as stream:
        read = MmapSource(stream)
        header = Reader(read).header()
        expected = TileType.WEBP if kind in {'terrain', 'sun'} else TileType.UNKNOWN if kind in {'snow', 'climate'} else TileType.MVT
        if header['tile_type'] != expected:
            raise ValueError(f'Unexpected {kind} tile format')
        count = 0
        for (z, _x, _y), data in all_tiles(read):
            if not header['min_zoom'] <= z <= header['max_zoom'] or not data:
                raise ValueError('Invalid tile or zoom')
            if header['tile_compression'] == Compression.GZIP:
                data = gzip.decompress(data)
            elif header['tile_compression'] != Compression.NONE:
                raise ValueError('Unsupported tile compression')
            if expected == TileType.WEBP:
                pixels(data)
            count += 1
        if not count or count != header['addressed_tiles_count']:
            raise ValueError('Archive tile count differs')


def verify(source):
    document = json.loads((source / 'release.json').read_bytes())
    previous = json.loads((source / 'previous.json').read_bytes()) if (source / 'previous.json').exists() else None
    changed = lambda name: previous is None or previous['files'].get(name) != document['files'][name]
    routing_changed = any(changed(name) for name in document['files'] if name.startswith('routing/'))
    selected = {name: entry for name, entry in document['files'].items()
                if changed(name) or name.endswith('.json') or routing_changed and name.startswith('routing/')}
    for name in document['files']:
        runtime.relative_path(name)
    indexes = {}
    for path in sorted((source / 'indexes').rglob('index.json')):
        item = json.loads(path.read_bytes())
        if item['kind'] in indexes:
            raise ValueError('Duplicate grid index')
        indexes[item['kind']] = item
    options = {key: document[key] for key in ('region', 'name', 'bounds', 'attribution', 'landcover_attribution')}
    files, release, catalog, search = index.compose(indexes, options)
    if document != {**release, 'grid': {'format': 2, 'zoom': 9, 'map_zoom': 11}, 'files': document['files']}:
        raise ValueError('Planner descriptor differs from its grid indexes')
    generated = {f"search/{document['region']}.grid.json", 'offline/catalog.json'}
    if set(document['files']) != files.keys() | generated or any(document['files'][name] != entry for name, entry in files.items()):
        raise ValueError('Planner files differ from their grid indexes')
    public = runtime.public_metadata(document)
    if {path.relative_to(source).as_posix() for path in (source / 'public').rglob('*') if path.is_file()} != public.keys():
        raise ValueError('Public pointers differ from the planner descriptor')
    for name, data in public.items():
        if (source / name).read_bytes() != data:
            raise ValueError('Public pointer identity differs')
    output = source / 'runtime'
    offline.materialize(source, output, {**document, 'files': selected}, ('',))
    catalog['files'] = {**files, f"search/{document['region']}.grid.json": document['files'][f"search/{document['region']}.grid.json"]}
    if json.loads((output / 'offline/catalog.json').read_bytes()) != catalog or json.loads((output / f"search/{document['region']}.grid.json").read_bytes()) != search:
        raise ValueError('Offline or search catalog differs from its grid indexes')
    graph = json.loads((output / 'routing/blocks.json').read_bytes())
    if graph != indexes['routing']['graph'] or runtime.digest(output / 'routing/blocks.json') != document['routing_package']:
        raise ValueError('Routing graph differs from the planner descriptor')
    for cell in indexes['routing']['cells']:
        path = output / 'offline' / cell['manifest']
        descriptor = json.loads(path.read_bytes())
        if runtime.digest(path) != cell['sha256'] or descriptor['source'] != graph['source'] or descriptor['data']['bounds'] != cell['bounds']:
            raise ValueError('Routing cell source or coverage differs')
    for cell in search['cells']:
        for name in cell['files']:
            if not changed('search/' + name): continue
            path = output / 'search' / name
            metadata = search_metadata(path, full=True)
            component = Path(name).parts[1]
            if (metadata['component'] != component or metadata['bounds'] != cell['bounds']
                    or metadata['osm_sha256'] != document['osm_sha256']
                    or metadata['time_zone'] != search['metadata']['time_zone']):
                raise ValueError('Search shard source, component or coverage differs')
    for name in files:
        if name.startswith('maps/') and name.endswith('.json') and Path(name).stem in index.MAP_KINDS:
            kind = Path(name).stem
            if json.loads((output / name).read_bytes()) != indexes[kind]['metadata']:
                raise ValueError('Map metadata differs from its grid index')
        if name.endswith('.pmtiles') and changed(name):
            archive(output / name, Path(name).parts[2])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    verify(parser.parse_args().source)


if __name__ == '__main__':
    main()
