"""POI records and contact metadata; independent from address production."""
import re
from records import has_poi, wikipedia_identity

def details(tags):
    def first(keys):
        return next((tags[k].strip() for k in keys if isinstance(tags.get(k), str) and tags[k].strip()), '')

    descriptions = ['description', 'description:en', 'description:de']
    descriptions += sorted(k for k in tags if k.startswith('description:') and k not in descriptions)
    return (first(['website', 'contact:website']), first(['phone', 'contact:phone']), first(descriptions))



def add(writer, p, record):
    if not has_poi(p):
        return
    ns, kind, source, lon, lat, city, postcode, bbox, region, context, country = record
    pid = writer.place(source, ns[0] if ns else kind.replace('_', ' '), ';'.join(ns), kind,
                 lon, lat, city, postcode, p.get('importance', 0) or 0, bbox, region, context, country,
                 p.get('extra', {}).get('cuisine', ''), p.get('extra', {}).get('opening_hours', ''),
                 *details(p.get('extra', {})))

    tags = p.get('extra', {})
    identities = set()
    qid = tags.get('wikidata', '')
    if re.fullmatch(r'Q[1-9]\d*', qid):
        identities.add(qid)
    for key, value in tags.items():
        if key == 'wikipedia' or key.startswith('wikipedia:'):
            if identity := wikipedia_identity(value, key.removeprefix('wikipedia:') if key != 'wikipedia' else ''):
                identities.add(identity)
    writer.db.executemany('INSERT OR IGNORE INTO place_identities VALUES (?,?)',
                         ((pid, identity) for identity in sorted(identities)))
