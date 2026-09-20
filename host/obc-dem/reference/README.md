# Reference DEMs for crest planes

`obc-dem surface --reference <dir>` reads a finer DEM and writes OBCT §9 crest planes with it.
The crest planes lift summits and ridge crests that the `2^9` µdeg lattice loses, and **only Peak
View reads them** — the native heights the route profile, the contours and the altimeter read do
not change.

A reference is optional per cell. Where the directory covers the box, crests are corrected; where
it does not, the cells come out byte-identical to a bake without it. National coverage may
therefore stop at a border with no special handling.

`fetch_reference.py` pulls a box from a national service and writes the WGS84 GeoTIFFs the baker
wants:

```sh
python3 fetch_reference.py --list
python3 fetch_reference.py --source auto --bbox 6.75,45.80,7.05,46.00 --out ref/
obc-dem surface native.obcd crest.obcd --reference ref/
```

`--bbox` is longitude first, like `obc-pack --bbox` and unlike `obc-dem --bbox`. `--source auto`
takes the finest source that covers the box. The script needs `rasterio`, `pyproj` and `numpy`.

## Sources with no key

These answer a plain HTTP request. `fetch_reference.py` knows them.

| key | country | step | product | licence | attribution |
| --- | --- | --- | --- | --- | --- |
| `ch` | Switzerland | 2 m | swissALTI3D | Open data, attribution required | `© swisstopo` |
| `fr` | France | 1 m | RGE ALTI (IGN) | Licence Ouverte | `© IGN` |
| `us` | United States | 1 m | 3DEP (USGS) | Public domain | `USGS 3DEP` |
| `no` | Norway | 1 m | NHM DTM (Kartverket) | CC BY 4.0 | `© Kartverket` |
| `es` | Spain | 5 m | MDT05 / PNOA LiDAR (IGN) | CC BY 4.0 | `© Instituto Geográfico Nacional` |
| `nl` | Netherlands | 0.5 m | AHN DTM (PDOK) | CC BY 4.0 | `© Rijkswaterstaat / AHN` |
| `de-nw` | Germany, North Rhine-Westphalia | 1 m | DGM1 (Geobasis NRW) | dl-de/zero-2-0 | `© Geobasis NRW` |

The attribution in the last column must travel with any map baked from that reference, next to the
Copernicus attribution the native heights already carry.

Germany publishes elevation per state, each with its own service. NRW is in the registry because
its WCS needs nothing else; the other states follow the same pattern and are a registry entry
each.

## Sources that need a key or a login

These are open data, but a download needs registration, so a bake cannot pull them unattended.
Fetch them by hand and put the GeoTIFFs in the `--reference` directory.

| country | step | product | how to get it |
| --- | --- | --- | --- |
| Denmark | 0.4 m | DHM/Terræn | Datafordeler account, WCS token |
| Sweden | 1 m | Markhöjdmodell (Lantmäteriet) | Geotorget account |
| Finland | 2 m | Korkeusmalli (NLS) | API key from the NLS file service |
| Austria | 1 m | per-state DGM (data.gv.at) | bulk download per state, no live service |
| Italy | 1–2 m | per-region DTM | regional portals, coverage is partial |
| United Kingdom | 1 m | LIDAR Composite DTM (EA) | DEFRA survey portal, England only |
| Canada | 1 m | HRDEM (NRCan) | open S3 bucket, per-project tiles |
| Australia | 1–5 m | ELVIS (Geoscience Australia) | ELVIS portal, per-area order |
| New Zealand | 1 m | LiDAR DEM (LINZ) | LINZ Data Service API key |

## What makes a good reference

- **A bare-earth DTM is safe.** Copernicus GLO-30 is a surface model, so it sits above a DTM over
  forest and buildings. A lift cannot be negative, so the baker simply does not lift there. On the
  bare rock that Peak View is about, the two models agree.
- **Five metres is enough.** The lift is quantised to 2 m and gated on convexity, so a 5 m
  reference recovers a summit as well as a 1 m one. Fetch the coarser product when it is faster.
- **A reference that is wrong low costs nothing.** RGE ALTI is weak over the Mont Blanc glaciers
  and reads below Copernicus there; the bake leaves those samples alone.
