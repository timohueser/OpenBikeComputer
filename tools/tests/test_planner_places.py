"""Place tiles keep the identities, names and coordinates of searchable rider places."""

import gzip
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

from tools import planner_places as places


class PlacesArchive(unittest.TestCase):
    def test_tiles_are_a_projection_of_the_search_database(self):
        from pmtiles.reader import MmapSource, all_tiles, Reader
        with tempfile.TemporaryDirectory() as directory:
            source, target = Path(directory) / 'pois.sqlite', Path(directory) / 'places.pmtiles'
            with sqlite3.connect(source) as db:
                db.executescript('CREATE TABLE metadata(key,value); CREATE TABLE places(source,kind,name,lon,lat);')
                db.executemany('INSERT INTO metadata VALUES (?,?)', [(k,json.dumps(v)) for k,v in {
                    'component':'pois', 'bounds':[7,47,9,49], 'osm_sha256':'a'*64, 'attribution':'OSM'}.items()])
                db.executemany('INSERT INTO places VALUES (?,?,?,?,?)', [
                    ('n1','campsite','Camp',7.9,47.9), ('w2','parking','Car park',7.9,47.9),
                    ('r3','drinking_water','Water',7.900001,47.900001), ('n4','hotel','Outside',10,50)])
            self.assertEqual(places.derive(source,target), {'places':2,'tiles':1})
            with target.open('rb') as stream:
                data=MmapSource(stream)
                self.assertEqual(Reader(data).metadata()['osm_sha256'],'a'*64)
                tiles=list(all_tiles(data))
            records=list(places.pois(gzip.decompress(tiles[0][1])))
            self.assertEqual([r[0] for r in records], [2**44+1,3*2**44+3])
            self.assertEqual([r[1] for r in records], [
                {'kind':'campsite','name':'Camp','lon':'7.9','lat':'47.9'},
                {'kind':'drinking_water','name':'Water','lon':'7.900001','lat':'47.900001'}])
            with sqlite3.connect(source) as db:
                db.execute('DELETE FROM places')
            empty = Path(directory) / 'empty.pmtiles'
            self.assertEqual(places.derive(source, empty), {'places': 0, 'tiles': 1})
            with empty.open('rb') as stream:
                self.assertEqual(list(places.pois(gzip.decompress(list(all_tiles(MmapSource(stream)))[0][1]))), [])
            with sqlite3.connect(source) as db:
                db.execute("UPDATE metadata SET value='\"addresses\"' WHERE key='component'")
            with self.assertRaisesRegex(ValueError,'POI search component'):
                places.derive(source,Path(directory)/'invalid.pmtiles')


if __name__ == '__main__':
    unittest.main()
