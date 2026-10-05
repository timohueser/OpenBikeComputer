"""Read static country data from a pinned Nominatim archive; no import or database."""
import argparse
import json
from pathlib import Path
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


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    export(args.archive, args.output)
