"""Address records own their street names and house-number index."""
import hashlib
from index import norm


def add(writer, p, record):
    ns, kind, source, lon, lat, city, postcode, bbox, region, context = record
    street = p.get('address', {}).get('street', '')
    house = p.get('housenumber', '')
    if kind == 'street' and ns:
        street = ns[0]
    if street and (house or kind == 'street'):
        key = (street, city, postcode, region)
        sid = writer.streets.get(key)
        if sid is None:
            stable = 's' + hashlib.sha1('|'.join(key).encode()).hexdigest()[:20]
            aliases = ';'.join(ns) if kind == 'street' else street
            sid = writer.place(stable, street, aliases, 'street', lon, lat, city, postcode,
                             0.05, bbox, region, context)
            writer.streets[key] = sid
        if house:
            writer.db.execute('INSERT INTO addresses VALUES (?,?,?,?,?)', (sid, norm(house), lon, lat, source))
