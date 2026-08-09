# obc-wx-bake fixtures

Real, unmodified upstream objects captured on 2026-08-09 UTC. They drive the deterministic
fixture cycles in `tests/cycle.rs`: same fixtures ⇒ byte-identical published tree, and every
corruption of them must publish nothing. Provenance follows the WX1 spike's discipline
(`host/obc-wx-source-spike/tests/fixtures/README.md`); terms for both sources are DWD Open Data,
CC BY 4.0.

## DWD RV composite

One complete immutable run tar (25 ODIM HDF5 members, leads 000..120 at 5 minutes):

- `composite_rv_20260809_1420.tar` — 2,539,520 bytes,
  sha256 `0b1696302476b3663ee7c942eff076934ac8aec441e0c21c6dfda3d0c014347a`.
  Retrieved 2026-08-09 14:24 UTC as
  `https://opendata.dwd.de/weather/radar/composite/rv/composite_rv_LATEST.tar`
  (upstream `Last-Modified` 14:23-range for the 14:20 run) and byte-compared equal against the
  immutable name
  `https://opendata.dwd.de/weather/radar/composite/rv/composite_rv_20260809_1420.tar`.

## DWD ICON-EU TOT_PREC

The complete retained lead set of the then-newest complete run (2026-08-09 06Z): the f000
baseline plus forward leads 001..012, each
`https://opendata.dwd.de/weather/nwp/icon-eu/grib/06/tot_prec/icon-eu_europe_regular-lat-lon_single-level_2026080906_FFF_TOT_PREC.grib2.bz2`,
retrieved 2026-08-09 14:24-14:56 UTC:

- `icon-eu-2026080906_000.grib2.bz2` … `icon-eu-2026080906_012.grib2.bz2` (13 files,
  4,582,350 bytes total). `f000` is the 198-byte all-zero accumulation baseline.
  `f001` is byte-identical to the WX1 spike's `icon-eu-20260809T06-f001.grib2.bz2` capture
  (sha256 `b5a0b2db80bc3aa3b22cda9ed6c282aaa0551a96aac4a9b4c632b83acd0589ff`), retrieved
  independently — the immutable object contract holding in practice.

Corrupt-upstream negatives are derived in-test by truncating/flipping these bytes; no corrupt
fixture is checked in.
