# European camping rulebook

Open `map.html` in a browser. The map works offline. Its source links need a connection.
Use the shelter selector to compare a small tent with shelterless bivouac.
Search for a country, region, park or site. Select a result to read its conditions and sources.

`data.json` is the editable rulebook. `data.schema.json` describes its structure.
The map embeds the same data, D3 and Natural Earth overview geometry.
The D3 licence is included in the HTML. Geometry credits are in the map and JSON.

After editing the JSON, update the map:

```sh
python3 data/wildcamping-europe/update_map.py
python3 data/wildcamping-europe/update_map.py --check
```

Colours apply only to each record's stated equipment, activity and area.
Markers are reference points, not legal boundaries or permission for a nearby pitch.
Vehicle parking and sleeping are outside this rulebook's scope.
Unverified claims and source conflicts stay explicit. Check current closures before travel.
The rulebook is a research input. Device map generation does not consume it.
