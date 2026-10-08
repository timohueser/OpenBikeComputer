"""Project prepared articles into the existing planner place database."""
import json
from pathlib import Path
from records import wikipedia_identity

# Device category numbers, mapped to the existing search vocabulary.
KINDS = ('attraction', 'castle', 'archaeological_site', 'monastery', 'church', 'pass', 'hut',
         'viewpoint', 'attraction', 'attraction', 'attraction', 'church', 'attraction',
         'attraction', 'attraction', 'spring')


def article(record):
    def attribution(value):
        return {key: value[key] for key in ('source_url', 'revision', 'license_url')}

    result = {'default_language': record['default_language'], 'variants': [
        {'language': variant['language'], 'text_pages': variant['text_pages'],
         'attribution': attribution(variant['attribution'])} for variant in record['variants']]}
    if photo := record.get('photo'):
        result['photo'] = {**attribution(photo['attribution']),
                           'online_url': photo.get('online_url'), 'credit': photo['credit'],
                           **{key: photo.get(key) for key in ('file_identity', 'page_revision', 'file_revision')}}
    return json.dumps(result, ensure_ascii=False, separators=(',', ':'), sort_keys=True)


def add(writer, landmarks=(), peaks=(), bounds=None):
    def within(lon, lat):
        return not bounds or bounds[0] <= lon <= bounds[2] and bounds[1] <= lat <= bounds[3]

    def attach(pid, record, landmark_id=None):
        writer.db.execute('INSERT OR REPLACE INTO place_content VALUES (?,?,?)',
                          (pid, article(record), landmark_id))

    for path in landmarks:
        content = json.loads(Path(path).read_text())
        aliases = {**content.get('aliases', {}),
                   **{wikipedia_identity(key): value for key, value in content.get('wikipedia_aliases', {}).items()}}
        for record in content['records']:
            qid = record['qid']
            identities = [qid, *(key for key, value in aliases.items() if value == qid)]
            matches = writer.db.execute('SELECT DISTINCT place_id FROM place_identities WHERE identity IN (%s)'
                                        % ','.join('?' for _ in identities), identities).fetchall()
            matches += writer.db.execute('SELECT id FROM place_records WHERE source=?', (qid,)).fetchall()
            if matches:
                for pid, in matches:
                    name, place_aliases = writer.db.execute('SELECT name,aliases FROM place_records WHERE id=?', (pid,)).fetchone()
                    names = sorted(set((place_aliases or name).split(';')) | {record['name']})
                    writer.db.execute('UPDATE place_records SET aliases=? WHERE id=?', (';'.join(names), pid))
                    attach(pid, record, qid)
                continue
            lon, lat = record['longitude'], record['latitude']
            if not within(lon, lat):
                continue
            pid = writer.place(qid, record['name'], record['name'], KINDS[record['category'] - 1],
                               lon, lat, '', '', 0, [lon, lat, lon, lat], '', '', '')
            attach(pid, record, qid)
    for path in peaks:
        content = json.loads(Path(path).read_text())
        records = {record['id']: record for record in content['records']}
        for association in content['associations']:
            # Summit positions and source IDs remain those of OSM, even when an article is shared.
            match = writer.db.execute('SELECT id FROM place_records WHERE source=? AND kind=?',
                                      (f"n{association['node_id']}", 'summit')).fetchone()
            if match:
                attach(match[0], records[association['article_id']])
