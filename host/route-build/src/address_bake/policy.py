"""Export Nominatim's country configuration without an import or a database."""
import argparse
import json
from pathlib import Path

from nominatim_db.config import Configuration
from nominatim_db.data import country_info


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    config = Configuration(None)
    country_info.setup_country_config(config)
    countries = {code: {key: value for key, value in props.items()
                        if key in ('names', 'languages', 'postcode')}
                 for code, props in country_info.iterate()}
    data = {'countries': countries,
            'levels': json.loads((config.config_dir / 'address-levels.json').read_text())}
    with args.output.open('x') as stream:
        json.dump(data, stream, ensure_ascii=False, sort_keys=True)
        stream.write('\n')


if __name__ == '__main__':
    main()
