# Train timetable probe

Use Python 3.11 or later and a local [MOTIS](https://github.com/motis-project/motis)
binary. Run the Python commands from the checkout root.

## Prepare

Download a GTFS ZIP from its publisher. Check its terms before use or
redistribution. These publishers provide inputs for the probe:

| Region | Publisher |
| --- | --- |
| Germany | [Regional and long-distance rail](https://gtfs.de/en/feeds/) |
| Switzerland | [National timetable](https://opentransportdata.swiss/en/cookbook/timetable-cookbook/gtfs/) |
| France | [SNCF timetable](https://transport.data.gouv.fr/datasets/horaires-sncf) |

Filter each mixed feed. The output path must not exist.

```sh
python3 -m tools.planner_trains prepare /data/full.zip /data/rail.zip
```

The command streams the CSV tables. It keeps route type 2 and extended types
100 to 117. It retains the trips, stop times, parent stations, service calendars,
calendar exceptions, frequencies, transfer rules and source attributions for
those routes. It removes shape references. Geometry, fares, pathways and
translations are outside this probe. The JSON report lists omitted tables,
row counts, file hashes, file sizes and bike carriage values.
It also lists feed metadata and operator URLs. An operator URL can be a
publisher fallback. It is not proof that the operator sells tickets there.
The filter uses the publisher's route types. Some publishers assign a rail
type to replacement buses. Check those services before product use.

## Import and run

Save this as `/data/config.yml`. Add one dataset per feed. Dataset names must
not contain an underscore. Each name becomes a prefix in the stop IDs.

```yaml
server:
  host: 127.0.0.1
  port: 8080
  n_threads: 2
timetable:
  first_day: TODAY
  num_days: 31
  railviz: false
  with_shapes: false
  merge_dupes_intra_src: true
  merge_dupes_inter_src: true
  datasets:
    rail:
      path: /data/rail.zip
limits:
  onetoall_max_travel_minutes: 1440
  onetoall_max_results: 100000
```

```sh
motis import -c /data/config.yml -d /data/prepared
motis server -d /data/prepared
```

This configuration loads timetable data only. It uses feed transfers and
nearby-stop links. It does not check walking paths against a street map.
Do not extend service calendars beyond their published dates.

## Measure

Save a JSON array as `/data/cases.json`. Obtain stop IDs from `stops.txt`.
Use a parent station where available. Set `start` to a time with an explicit
UTC offset inside the imported timetable. Add `to` to retrieve journey legs.

```json
[{"station":"rail_STATION_ID","start":"2026-10-05T08:00:00+02:00",
  "max_minutes":480,"changes":2,"to":"rail_DESTINATION_ID"}]
```

```sh
python3 -m tools.planner_trains bench --cases /data/cases.json --runs 21
```

The command reports the first request and the warm median and p95. Times
include local HTTP, response transfer and JSON decoding. Counts are stop
records, including platforms and copies in other feeds. They are not unique
stations. A fixed start includes the wait for the first train in the budget.

For a departure window, save one case object with `start` and `end`, then run:

```sh
python3 -m tools.planner_trains window --case /data/window.json
```

The window includes `start` and excludes `end`. This baseline queries each
distinct origin departure minute and keeps the shortest result per stop.
It covers the origin station group from the departure board. It is not an
exact profile search with arbitrary access walks. Its work grows with the
number of departures. Each result keeps the query time for journey lookup.
MOTIS uses minute resolution. Use minute-aligned window bounds.

Run these probes against a local service. The public Transitous service
requires prior contact for expensive routing and isochrone requests.

## Check

```sh
pip install -r tools/requirements-test.txt
python3 -m unittest discover -s tools/tests -v
obc suites check
```
