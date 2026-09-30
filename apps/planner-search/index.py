"""Build name and spelling indexes from the package's place records."""
import argparse
import json
import re
import sqlite3
import unicodedata
from pathlib import Path

ROOT = Path(__file__).parent
ADDRESS_TERMS = json.loads((ROOT / 'web/address-terms.json').read_text())


def norm(s):
    s = s.lower().replace('ß', 'ss')
    s = ''.join(c for c in unicodedata.normalize('NFKD', s) if not unicodedata.combining(c))
    return re.sub(r'[\W_]+', ' ', s).strip()


def dictionary(rows):
    entries = {}
    for row in rows:
        for word in row:
            key, canonical = norm(word), norm(row[0])
            entries[key] = None if key in entries and entries[key] != canonical else canonical
    return entries


WORDS = dictionary(ADDRESS_TERMS['words'])
SUFFIXES = sorted(((k, v) for k, v in dictionary(ADDRESS_TERMS['suffixes']).items()
                   if len(k) >= 2 and v), key=lambda pair: -len(pair[0]))


def street_norm(s):
    out = []
    for word in norm(s).split():
        if WORDS.get(word):
            out.append(WORDS[word])
            continue
        for suffix, full in SUFFIXES:
            if len(word) > len(suffix) + 1 and word.endswith(suffix):
                word = word[:-len(suffix)] + ' ' + full
                break
        out.append(word)
    return ' '.join(out)


def index_search(db):
    db.executescript('''
      DROP TABLE IF EXISTS main.vocabulary;
      DROP TABLE IF EXISTS main.terms;
      DROP TABLE IF EXISTS main.names;
      DROP TABLE IF EXISTS main.compact_names;
      DROP TABLE IF EXISTS main.fuzzy;
      DROP TABLE IF EXISTS main.lexicon;
      CREATE TABLE names(term TEXT,place_id INTEGER,PRIMARY KEY(term,place_id)) WITHOUT ROWID;
      CREATE TABLE compact_names(term TEXT,place_id INTEGER,PRIMARY KEY(term,place_id)) WITHOUT ROWID;
      CREATE VIRTUAL TABLE terms USING fts5(name,context,content='',detail=column,
        tokenize='unicode61 remove_diacritics 2',prefix='3');
    ''')
    name_rows, compact_rows, text_rows = [], [], []

    def flush():
        db.executemany('INSERT OR IGNORE INTO names VALUES (?,?)', name_rows)
        db.executemany('INSERT OR IGNORE INTO compact_names VALUES (?,?)', compact_rows)
        db.executemany('INSERT INTO terms(rowid,name,context) VALUES (?,?,?)', text_rows)
        name_rows.clear()
        compact_rows.clear()
        text_rows.clear()

    for pid, name, aliases, kind, context in db.execute('SELECT id,name,aliases,kind,context FROM places'):
        forms = set()
        for s in [name, *aliases.split(';')]:
            if not s:
                continue
            for variant in [s, s.lower().replace('ä', 'ae').replace('ö', 'oe').replace('ü', 'ue')]:
                forms.add(norm(variant))
                if kind == 'street':
                    forms.add(street_norm(variant))
        name_rows.extend((term, pid) for term in forms)
        compact_rows.extend((term.replace(' ', ''), pid) for term in forms)
        text_rows.append((pid, ' '.join(sorted(forms)), norm(context)))
        if len(text_rows) >= 10000:
            flush()
    flush()
    db.execute("INSERT INTO terms(terms) VALUES('optimize')")
    print('Building spelling dictionary', flush=True)
    db.executescript('''
      CREATE TABLE lexicon(term TEXT);
      INSERT INTO lexicon SELECT DISTINCT term FROM compact_names WHERE length(term) BETWEEN 5 AND 80;
      CREATE VIRTUAL TABLE fuzzy USING fts5(term,content='lexicon',detail=none,tokenize='trigram');
      INSERT INTO fuzzy(fuzzy) VALUES('rebuild');
      INSERT INTO fuzzy(fuzzy) VALUES('optimize');
      ANALYZE main;
    ''')
    db.commit()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('databases', type=Path, nargs='+')
    for path in parser.parse_args().databases:
        print(f'Indexing {path}', flush=True)
        with sqlite3.connect(path) as db:
            db.executescript('PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-32768; PRAGMA temp_store=FILE;')
            index_search(db)
            db.execute('VACUUM')
        print(f'{path}: {path.stat().st_size / 1e6:.1f} MB', flush=True)
