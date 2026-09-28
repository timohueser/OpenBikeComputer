# Search spike

Run from this folder. Node 24 or later supplies SQLite. Python builds the data.
The browser needs WebAssembly, service workers and OPFS. Use one playground tab
per browser profile: the SQLite storage pool holds an exclusive file handle.

## Build and run

```sh
npm ci
uv venv .venv
uv pip install --python .venv/bin/python -r requirements.txt
mkdir -p data parser
curl -fL --retry 3 -o data/germany.jsonl.zst \
  https://download1.graphhopper.com/public/europe/germany/photon-dump-germany-1.0-latest.jsonl.zst
.venv/bin/python build.py data/germany.jsonl.zst
gzip -k -6 data/baden-wuerttemberg.sqlite
gh release download spike/query-parser-v2 -R timohueser/OpenBikeComputer \
  -p query-parser-v2-int8.tar.gz -D data
tar -xzf data/query-parser-v2-int8.tar.gz -C parser
python3 setup.py
npm start
```

Open <http://localhost:8780>. The server binds to localhost only. `PORT` changes
the port. The model release needs repository access. `setup.py` reads the
`spike/query-parser-v2` git tag for its label list and category vocabulary.

The builder refuses to replace an existing database. Remove only its generated
`data/germany.sqlite` and `data/baden-wuerttemberg.sqlite` files before a rebuild.
Both packages come from the same input stream. Recreate the gzip file after a
rebuild. Keep the input dump to repeat a build against the same data snapshot.
Use `--output data/rebuilt` to build beside the active packages.
To change only text matching, stop the server and run `python3 index.py
data/germany.sqlite data/baden-wuerttemberg.sqlite`. This rebuilds the name
indexes from stored place records. Recreate the gzip file and restart the server.

Select **Download Baden-Württemberg** to store the database and query model in
the browser. Uncheck **Use Germany server** to limit search to that package.
Browser network-offline mode also tests a reload without the server. Detailed
map tiles need a connection; the offline view keeps region outlines and results.
Browser storage is separate from the files in `data/`.

Try these inputs:

| Input | Setup |
| --- | --- |
| `bakery` | Freiburg map view |
| `Kandel` | Compare Freiburg and Karlsruhe map views |
| `München`, `Munich`, `Muenchen` | Germany server enabled |
| `Deutsches Museum München` | Germany server enabled |
| `Platz der Republik 1 Berlin` | Germany server enabled |
| `Kaiser Joseph Straße 242 Freiburg` | Baden-Württemberg downloaded |
| `Habsburgerstr. 10 Freiburg` | Baden-Württemberg downloaded |
| `Media Markt`, `NediaMarkt`, `Mdeia Mrkt` | Freiburg map view |
| `pizza`, `Döner` | Freiburg map view |
| `Döner in Teningen` | Baden-Württemberg downloaded |
| `bakeries in Munich` | Germany server enabled |
| `bakeries near me` | Select **Use my location** first |
| `bakeries at the end of day three` | Load the sample route |
| `bakeries in the first half of the route` | Load the sample route |

The sample GPX comes from the route-import fixture. Its three day boundaries are
illustrative. Uploaded GPX files also receive three illustrative days. The line
does not pass through a router. Corridor distance is geometric, not a riding
detour. A day-end search uses a 3 km radius; a route search uses a 1 km corridor.

## Checks

```sh
npm test
node test/data.mjs
npx playwright install chromium
npm run test:browser
```

The data and browser suites need the server. `CHROMIUM_PATH` can select an
installed Chromium executable. The browser suite uses a fresh browser profile.
It checks a complete network-offline reload, the model, search, result merging
and route filtering. Generated measurements and screenshots go to the ignored
`test-output/` directory. Run `obc suites check` from the checkout after test edits.

## Files and limits

| File | Use |
| --- | --- |
| `build.py` | Stream the prepared OSM dump into both SQLite packages |
| `index.py` | Build text, compact-name and spelling indexes |
| `web/text.mjs` | Normalize text and compare spellings |
| `web/engine.mjs` | Shared retrieval, ranking, addresses and geographic filters |
| `server.mjs` | Serve Germany searches and local playground assets |
| `web/db-worker.mjs` | Query the downloaded SQLite file in browser OPFS |
| `web/parser-worker.mjs` | Run mmBERT and decode the search subset of its labels |
| `web/sw.js` | Cache the app shell, runtime and model for offline reloads |

- The prepared data comes from [Photon's download service](https://download1.graphhopper.com/public/europe/germany/).
  The serving process does not use Photon or OpenSearch. A build directly from
  raw OSM is not part of this spike.
- Search data is © OpenStreetMap contributors, under ODbL 1.0. See the
  [OSM attribution and licence](https://www.openstreetmap.org/copyright).
  The parser uses the archived MIT mmBERT model. Its category terms come from
  the iD tagging schema under ISC, with extensions in the archived parser.
  `web/address-terms.json` contains the German `street_types` and
  `concatenated_suffixes_separable` dictionaries from
  [libpostal](https://github.com/openvenues/libpostal/tree/25099c506612b34b23b1bfe286ca6321fcf06f35/resources/dictionaries/de).
  See `LICENSE.libpostal`. The full libpostal parser is not bundled.
- The browser and server run the same JavaScript search code. This does not
  measure native iPhone performance or provide the final Rust integration.
- The parser adapter supports place search, category search, named areas, near
  me, route corridors, route halves and numbered day ends. It does not port the
  full archived decoder. Unsupported opening-hours and before/after filters
  return a visible message. Other request types fall back to text search.
- City filtering uses prepared address membership and the place bounds.
  Missing or inconsistent OSM city fields can omit results. No address
  interpolation or extra address source is used.
- A compact name index ignores spaces and punctuation. Trigrams retrieve up to
  256 spelling candidates. Edit distance permits one edit for 5–8 letters and
  two for longer names, including adjacent transpositions and first-letter
  errors. Up to eight corrections are searched. This is bounded retrieval;
  it can miss a match and does not guarantee spelling correction.
- German street matching uses the shared address dictionary on both indexed
  streets and queries. Ambiguous abbreviations are not expanded. Address
  ranking uses the map centre and the mapped house location. An explicit city
  still restricts the text match.
- Summits and passes receive a small ranking preference. Nearby street entries
  with the same name share one result. Summits and passes remain separate.
- Results with equivalent normalized names, categories and text-match quality
  are ordered by distance. This applies before result limits and after merging
  offline and server results. The inspector shows the base relevance score and
  the distance-order rule. Importance still ranks unlike destinations.
- Pizza and kebab searches use OSM cuisine tags and food business names. They
  keep the same geographic filters as other categories. Missing tags and names
  can still omit a business. The inspector shows the match source.
- The interface shows six suggestions or twenty submitted results, with more
  results up to one hundred. Candidate retrieval is bounded. Broad name queries
  do not enumerate every match. The inspector shows the ranking components.
- The service worker refreshes app code online and keeps it for offline use.
  Vendor assets stay cached. After a runtime or model change, clear Cache
  Storage. The OPFS database can stay. After data changes, download it again.
- Germany data tests do not establish worldwide coverage or world-scale memory
  use. The VPS limit needs a Linux load test with the final data and traffic.
