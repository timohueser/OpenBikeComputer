"""Shared source classification for independently built search components."""
import json
import re
from urllib.parse import parse_qs, unquote, urlsplit
from pathlib import Path

RIDER_KINDS = {kind for category in json.loads((Path(__file__).resolve().parents[2] /
    'builder/web/src/lib/planner/poi-kinds.json').read_text()).values() for kind in category['kinds']}

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


# A place that the query language finds by kind stays without a name; a settlement does not.
_CONTRACT = json.loads((Path(__file__).parent / 'query/contract.json').read_text())
SERVICES = set(_CONTRACT['data']) - set(_CONTRACT['kinds']['town']['data'])


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
    return kind != 'street' and (bool(ns) or kind in SERVICES | RIDER_KINDS) and not (
        p['osm_key'] == 'building' and p.get('housenumber') and not ns)


def wikipedia_identity(value, language=''):
    if not isinstance(value, str):
        return None
    value = value.strip()
    if value.startswith(('https://', 'http://')):
        url = urlsplit(value)
        host = url.hostname or ''
        if not host.endswith('.wikipedia.org') or url.fragment:
            return None
        language = host.removesuffix('.wikipedia.org')
        value = unquote(url.path.removeprefix('/wiki/')) if url.path.startswith('/wiki/') else parse_qs(url.query).get('title', [''])[0]
    elif not language:
        language, _, value = value.partition(':')
    if not re.fullmatch(r'[a-z][a-z0-9-]*', language) or not value or '#' in value:
        return None
    return f"{language}:{value.replace('_', ' ').strip()}"
