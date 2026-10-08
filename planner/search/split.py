"""Split an enriched OSM search dump once into independent POI and address inputs."""
import argparse
from contextlib import ExitStack
import io
import json
from pathlib import Path
import sys

from records import has_addresses, has_poi


def split_lines(lines, outputs):
    membership = {'pois': has_poi, 'addresses': has_addresses}
    counts = dict.fromkeys(membership, 0)
    for line in lines:
        obj = json.loads(line)
        for component, output in outputs.items():
            if obj['type'] == 'Place':
                places = [p for p in obj['content'] if membership[component](p)]
                if not places:
                    continue
                value = {**obj, 'content': places}
                counts[component] += len(places)
            else:
                value = obj
            output.write(json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode() + b'\n')
    return counts


def split(source, destination):
    import zstandard

    destination.mkdir(parents=True, exist_ok=True)
    paths = {component: destination / f'{component}.jsonl.zst' for component in ('pois', 'addresses')}
    if any(path.exists() for path in paths.values()):
        raise ValueError('Choose a fresh split output directory.')
    try:
        with ExitStack() as stack:
            raw = stack.enter_context(source.open('rb'))
            stream = stack.enter_context(zstandard.ZstdDecompressor().stream_reader(raw))
            outputs = {component: stack.enter_context(zstandard.ZstdCompressor().stream_writer(
                stack.enter_context(path.open('wb')))) for component, path in paths.items()}
            return split_lines(io.BufferedReader(stream), outputs)
    except BaseException:
        for path in paths.values():
            path.unlink(missing_ok=True)
        raise


def step():
    """The `obc data` step `planner/search/records`: the records of `planner/search/dump`."""
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
    from tools import step_request

    request = step_request.read()
    dump = Path(request['layers']['planner/search/dump']['search.jsonl.zst'])
    step_request.metrics(request, split(dump, Path(request['output'])))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('destination', type=Path)
    args = parser.parse_args()
    print(json.dumps(split(args.source, args.destination)))


if __name__ == '__main__':
    step() if sys.argv[1:] == ['--step'] else main()
