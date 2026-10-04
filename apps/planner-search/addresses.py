"""Address records own their street names and house-number index."""
import hashlib
import json
from dataclasses import dataclass
from index import norm


@dataclass(slots=True)
class Street:
    id: int
    representative: tuple
    lon: float
    lat: float
    bbox: list
    context: str
    aliases: set | None = None


def add(writer, p, record):
    ns, kind, source, lon, lat, city, postcode, bbox, region, context, country = record
    street = p.get('address', {}).get('street', '')
    house = p.get('housenumber', '')
    if kind == 'street' and ns:
        street = ns[0]
    if street and (house or kind == 'street'):
        key = (street, city, postcode, region, country)
        group = writer.streets.get(key)
        priority = (kind != 'street', p['object_type'], p['object_id'], house, lon, lat, context)
        if group is None:
            stable = 's' + hashlib.sha1(json.dumps(key, ensure_ascii=False, separators=(',', ':')).encode()).hexdigest()[:20]
            sid = writer.place(stable, street, street, 'street', lon, lat, city, postcode,
                             0.05, bbox, region, context, country)
            group = Street(sid, priority, lon, lat, list(bbox), context)
            writer.streets[key] = group
        else:
            group.bbox = [min(group.bbox[0], bbox[0]), min(group.bbox[1], bbox[1]),
                          max(group.bbox[2], bbox[2]), max(group.bbox[3], bbox[3])]
            if priority < group.representative:
                group.representative, group.lon, group.lat, group.context = priority, lon, lat, context
        if kind == 'street' and any(name != street for name in ns):
            if group.aliases is None:
                group.aliases = {street}
            group.aliases.update(ns)
        if house:
            writer.db.execute('INSERT INTO addresses VALUES (?,?,?,?,?)', (group.id, norm(house), lon, lat, source))


def finish(writer):
    for (street, city, postcode, region, country), group in writer.streets.items():
        aliases = ';'.join(sorted(group.aliases)) if group.aliases else None
        writer.db.execute('UPDATE place_records SET aliases=?,lon=?,lat=?,context_id=?,west=?,south=?,east=?,north=? WHERE id=?',
                          (aliases, group.lon, group.lat,
                           writer.context_id(city, postcode, region, group.context, country), *group.bbox, group.id))
