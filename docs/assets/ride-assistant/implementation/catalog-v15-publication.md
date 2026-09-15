# Public v15 catalog evidence

The Baden-Württemberg catalog now uses OBCM v15. It was built from the retained real OSM and
Copernicus DEM inputs with the shipping baker. The build completed all five plans: 215 cells
across four bands, one region, two skins, and 28 terrain cells. Local `obc-bake verify` passed.

The publisher verified 501 objects before it published the catalog root last. The complete
publication contains 502 objects and 885,920,706 bytes. The root SHA-256 is
`7d02c4c95cb81efde53c618c34dbb7cf52a2e98c3832daab2be489cd9f4a7497`.

The first publication used an incomplete URL prefix. A public root check found that its object
URLs omitted `cell-catalog`. Publication was repeated with the full public base URL below. No map
was rebaked. The corrected root and its object URLs were then checked through the public domain.

```sh
obc-bake publish catalog-v15-local \
  --base-url https://maps.openbikecomputer.com/cell-catalog --target r2 --dry-run
obc-bake publish catalog-v15-local \
  --base-url https://maps.openbikecomputer.com/cell-catalog --target r2
python3 host/obcm-assemble/dev/fetch_region.py \
  europe/germany/baden-wuerttemberg catalog-v15-public-coarse --band coarse
```

The existing region fetcher downloaded all nine coarse cells into an empty directory. Every
content hash matched the catalog, and every cell header was OBCM v15. This checks the public root,
region selection, band index, and immutable cell URLs. No assembly or resource image was run.
The public v16 successor is held until the v15 parent changes pass their gates.
