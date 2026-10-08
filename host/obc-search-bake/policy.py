"""Read static country data from a pinned Nominatim archive; no import or database."""
import argparse
import json
from pathlib import Path
import sys
import zipfile

import yaml


def export(archive, output):
    prefix = 'nominatim_db/resources/'
    with zipfile.ZipFile(archive) as bundle:
        class Loader(yaml.SafeLoader):
            pass
        def include(loader, node):
            name = loader.construct_scalar(node)
            if not name.startswith('country-names/') or '..' in name.split('/'):
                raise ValueError('Invalid country data include')
            return yaml.load(bundle.read(prefix + 'settings/' + name), Loader=Loader)
        Loader.add_constructor('!include', include)
        countries = yaml.load(bundle.read(prefix + 'settings/country_settings.yaml'), Loader=Loader)
        for code, props in countries.items():
            names = {}
            for key, value in props.get('names', {}).items():
                if isinstance(value, str):
                    names[key] = value
                else:
                    for language, name in value.items():
                        names[key if language == 'default' else key + ':' + language] = name
            countries[code] = {'names': names, 'postcode': props.get('postcode')}
        policy = {'countries': countries, 'levels': json.loads(bundle.read(prefix + 'settings/address-levels.json'))}
        output.mkdir(parents=True, exist_ok=True)
        (output / 'policy.json').write_text(json.dumps(policy, ensure_ascii=False, sort_keys=True) + '\n')
        (output / 'country_osm_grid.sql.gz').write_bytes(bundle.read(prefix + 'country_osm_grid.sql.gz'))


def step():
    """The `obc data` step `planner/search/policy`: the policy and the country grid of the
    `nominatim-country-data` snapshot."""
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
    from tools import step_request

    request = step_request.read()
    (archive,) = step_request.files(request, 'nominatim-country-data').values()
    export(archive, Path(request['output']))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    export(args.archive, args.output)


if __name__ == '__main__':
    step() if sys.argv[1:] == ['--step'] else main()
