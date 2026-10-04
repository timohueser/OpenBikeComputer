"""Shared source classification for independently built search components."""
def values(d, keys):
    out = []
    for k in keys:
        v = d.get(k, '')
        out.extend(v if isinstance(v, list) else [v])
    return list(dict.fromkeys(str(v) for v in out if v))


def names(d):
    return values(d, ['name', 'name:de', 'name:en', 'name:fr', 'name:it', 'name:es',
                      'name:nl', 'alt_name', 'loc_name', 'short_name', 'official_name', 'int_name'])


def category(p):
    k, v = p['osm_key'], p['osm_value']
    if k == 'mountain_pass':
        return 'pass'
    if v == 'yes':
        return k
    if p.get('address_type') in ('city', 'town', 'village', 'hamlet', 'suburb', 'district', 'state', 'country'):
        return p['address_type']
    if k == 'boundary':
        return 'locality'
    if p.get('address_type') == 'street':
        return 'street'
    return {'peak': 'summit', 'saddle': 'pass', 'camp_site': 'campsite',
            'alpine_hut': 'hut', 'wilderness_hut': 'hut', 'bicycle': 'bike_shop',
            'bicycle_repair_station': 'repair_station', 'station': 'train_station',
            'water': 'lake', 'doctors': 'doctor', 'charging_station': 'charging',
            'ferry_terminal': 'ferry', 'public_bath': 'shower'}.get(v, v)


SERVICES = {'bakery', 'supermarket', 'convenience', 'campsite', 'hotel', 'hostel', 'hut',
    'guest_house', 'drinking_water', 'water_point', 'spring', 'pharmacy',
    'bike_shop', 'repair_station', 'restaurant', 'cafe', 'shelter', 'toilets',
    'train_station', 'museum', 'viewpoint', 'pass', 'summit', 'fast_food',
    'water_tap', 'fountain', 'motel', 'butcher', 'marketplace', 'fuel',
    'bar', 'ice_cream', 'hospital', 'doctor', 'clinic', 'charging', 'shower',
    'laundry', 'atm', 'bus_stop', 'ferry', 'lake', 'beach', 'swimming_pool',
    'castle', 'church', 'monastery', 'ruins', 'waterfall', 'tower', 'bridge'}


def usable(p):
    return not (p['osm_key'] == 'place' and p['osm_value'] == 'postcode' and not p.get('object_type'))


def has_addresses(p):
    if not usable(p):
        return False
    street = p.get('address', {}).get('street', '')
    if category(p) == 'street' and names(p.get('name', {})):
        street = names(p.get('name', {}))[0]
    return bool(street and (p.get('housenumber') or category(p) == 'street'))


def has_poi(p):
    if not usable(p):
        return False
    kind, ns = category(p), names(p.get('name', {}))
    return kind != 'street' and (bool(ns) or kind in SERVICES) and not (
        p['osm_key'] == 'building' and p.get('housenumber') and not ns)
