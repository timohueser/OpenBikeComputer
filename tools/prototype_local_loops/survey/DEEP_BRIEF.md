# Brief: deep search for open data sources of local hiking routes

Desk research, read-only. Goal: a complete inventory, for the regions you are given, of every
dataset that holds NAMED LOCAL WALKING ROUTES as open data that a commercial app may reuse. The
routes we want are day walks and day hikes of about 3 to 25 km, mostly loops: the routes that a
municipality, a tourism office, a park or a hiking federation signs, registers, certifies or
recommends. A first shallow pass found some sources; your job is the careful pass.

## Where such data lives: work through this list for EVERY region

Search in the local language, with the local words for hiking trail, loop, themed trail, local
route, trail register (for example Wanderweg, Rundweg, Themenweg; sentier, boucle, PDIPR,
randonnée; sendero, PR, SL; sentiero, anello, rete escursionistica; wandelroute; turistická trasa).

1. The national open data portal, filtered to the region, and the regional open data portal.
   Many run CKAN (`/api/3/action/package_search?q=...`) or udata; use the API when robots.txt
   allows it, else the search page.
2. The regional geoportal or spatial data infrastructure (INSPIRE catalogue, state GIS, cadastre
   or mapping agency): layers of hiking routes, themed trails, municipal walks.
3. The regional or state tourism data hub (the database that local tourism offices type into) and
   its open data pool, if it has one.
4. Trail registers: the regional hiking federation or club, the official register of approved or
   homologated trails, the regional parks, national parks, nature parks, forestry service.
5. Certification and quality schemes for trails in that country.
6. Province, county, department or district level, where the region delegates trails to it.
7. Large municipalities and inter-municipal bodies, only if the levels above gave nothing.

## For each dataset you find, record

- `country`, `region`, `dataset`, `publisher`, `url` (the dataset page).
- `licence`: exactly as the dataset page, its metadata or its API states it. `licence_url`: where
  you read it. Null when none is stated. Never infer a licence from the publisher or the portal.
- `commercial_reuse`: yes, no or unclear. Note share-alike and attribution wording in `notes`.
- `kind`: `registered local routes`, `certified routes`, `municipal or tourism-office routes`,
  `themed trails`, `path network` (segments, signpost network, rights of way), `long trails only`
  or `mixed`.
- `has_line`: yes, no, some. `format`. `fetch_url`: the direct download or service URL.
- `access`: `open download`, `open service` (WFS, OGC API, ArcGIS REST), `free account`,
  `api key`, `signed agreement`, `on request`, or `web list only`.
- `routes`: the number of walking routes, only when a page, a file or a query states or returns
  it, with `count_source` and `count_method`. For an open service, a count query is welcome (WFS
  `resultType=hits`, ArcGIS `returnCountOnly=true`, OGC API `numberMatched`). You may download a
  file of at most 20 MB to count routes; say so. Otherwise null.
- `fields`: which of name, length, duration, difficulty, description, link the records carry.
- `verified`: `direct` when you opened the dataset page or called the service yourself, `lead`
  when you only have a search result or a catalogue pointer.
- `reachable`: false when the host reset, timed out or returned an error twice from here; the
  owner can then try by hand. Keep the entry as a `lead`.
- `notes`.

Also write, for every region, one entry `{"country", "region", "searched": [the places and query
words you tried], "result": "sources found" | "path network only" | "nothing found"}`. An honest
"nothing found, searched X, Y, Z" is a result. A region you did not get to is `"not searched"`.

## Rules

- Fetch with curl in Bash: `curl -sS -m 30 -A 'route-index-trial/0.1' URL`. The WebFetch tool
  cannot resolve most of these hosts in this session; do not conclude a host is unreachable from
  WebFetch alone. On a reset or a timeout, wait 10 seconds and try once more.
- Before the first request to a host, read its `/robots.txt` and obey it for the paths you call,
  API paths included. At most one request per second per host.
- Do not take route data from route platforms (outdooractive, komoot, alltrails, wikiloc,
  visorando, bergfex). Note it when an open dataset is fed from one or links to one.
- Do not invent a licence, a number or a URL. Null, "lead" and "nothing found" are good answers.
- Spend your effort where the data is: about 10 tool calls for a region with sources, 4 for a
  region that gives nothing. Stop at about 90 tool calls in total and report what is left.
- If you are given a file of earlier leads, start from it: verify each lead directly, correct it,
  then go beyond it.

## Output

Write ONE JSON file at the output path you are given: a list of the dataset entries and the
region entries. Then reply with at most 300 words: the best sources with their licence, access and
count; the regions with nothing; the regions you did not get to; your main doubts. Do not paste
the JSON. Write only that file and do not touch git.
