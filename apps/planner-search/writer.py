"""Common SQLite record and locality context writer."""
from records import category, names, values
from storage import create, finish


class Writer:
    def __init__(self, name, output, component='all'):
        if component not in ('all', 'pois', 'addresses'):
            raise ValueError('Invalid search component')
        self.component = component
        self.path = output / f'{name}.sqlite'
        self.db = create(self.path)
        self.streets = {}
        self.contexts = {}
        self.next_id = 2**52 if component == 'addresses' else 0

    def place(self, source, name, aliases, kind, lon, lat, city, postcode, importance, bbox, region, context, country, cuisine='', opening_hours='', website='', phone='', description=''):
        self.next_id += 1
        if self.next_id >= (2**52 if self.component == 'pois' else 2**53):
            raise ValueError('Search identity exceeds the JavaScript integer range')
        key = (city, postcode, region, context, country)
        context_id = self.contexts.get(key)
        if context_id is None:
            context_id = len(self.contexts) + 1
            self.contexts[key] = context_id
            self.db.execute('INSERT INTO place_contexts VALUES (?,?,?,?,?,?)', (context_id, *key))
        self.db.execute('INSERT INTO place_records VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
                        (self.next_id, source, name, None if aliases == name else aliases,
                         kind, lon, lat, context_id, importance, *bbox, cuisine, opening_hours, website, phone, description))
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
        country_code = p.get('country_code', '')
        country = 'Deutschland Germany' if country_code == 'de' else ' '.join(values(a, ['country', 'country:en']))
        context = ' '.join(values(a, ['city', 'city:de', 'city:en', 'district', 'locality',
                                     'county', 'state', 'street']) + [postcode, country])
        source = f"{p['object_type'].lower()}{p['object_id']}"
        bbox = p.get('bbox', [lon, lat, lon, lat])
        bbox = [min(bbox[0], bbox[2]), min(bbox[1], bbox[3]), max(bbox[0], bbox[2]), max(bbox[1], bbox[3])]
        kind = category(p)
        record = (ns, kind, source, lon, lat, city, postcode, bbox, region, context, country_code)
        if self.component in ('all', 'addresses'):
            from addresses import add
            add(self, p, record)
        if self.component in ('all', 'pois'):
            from pois import add
            add(self, p, record)

    def finish(self, meta):
        finish(self.db, self.path, {**meta, 'component': self.component})
