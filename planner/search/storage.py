"""Write the shared regional search schema and indexes."""
import json
import sqlite3
from pathlib import Path

from index import index_search

ROOT = Path(__file__).parent


def create(path):
    if path.exists():
        raise ValueError(f'{path} exists; choose a fresh output directory.')
    path.parent.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(path, uri=True)
    db.executescript('''PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;
        PRAGMA cache_size=-32768; PRAGMA temp_store=FILE;''')
    db.executescript((ROOT / 'schema.sql').read_text())
    return db


def finish(db, path, meta):
    counts = dict(db.execute('SELECT kind,count(*) FROM places GROUP BY kind'))
    counts['address'] = db.execute('SELECT count(*) FROM addresses').fetchone()[0]
    print(f'Indexing {path.name}: {sum(counts.values())-counts["address"]:,} places, {counts["address"]:,} addresses', flush=True)
    db.commit()
    db.executescript((ROOT / 'indexes.sql').read_text())
    index_search(db)
    meta = {**meta, 'schema': 6, 'counts': counts}
    db.executemany('INSERT INTO metadata VALUES (?,?)', ((k, json.dumps(v, ensure_ascii=False)) for k, v in meta.items()))
    db.commit()
    db.close()
    print(f'{path.name}: {path.stat().st_size / 1e6:.1f} MB', flush=True)
