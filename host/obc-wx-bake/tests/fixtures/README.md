# obc-wx-bake fixtures

Real, unmodified upstream objects (and exact upstream byte ranges) captured on 2026-08-09 UTC —
plus, since WXR6, two **tile crops** of real objects, which are the same idea applied to a format
whose payload is addressable in blocks rather than in byte ranges (see below). They drive the
deterministic fixture cycles in `tests/cycle.rs`, `tests/us_gfs_cycle.rs` and `tests/opera.rs`:
same fixtures ⇒ byte-identical published tree, and every corruption of them must publish nothing.

Provenance discipline: every entry below records the exact retrieval URL, the byte range where
one was used, the length and the SHA-256 of the checked-in bytes. Terms are DWD Open Data
(CC BY 4.0) for the German sources, NOAA Open Data Dissemination (public-use U.S. government
data, no endorsement implied) for the NOAA ones and CC BY 4.0 for EUMETNET OPERA; the license
record lives in
[`docs/decisions/WX1-weather-source-contracts.md`](../../../../docs/decisions/WX1-weather-source-contracts.md).

Total checked-in fixture bytes: 18,850,678.

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

## NOAA MRMS PrecipRate (CONUS observation)

One immutable two-minute object from
`https://noaa-mrms-pds.s3.amazonaws.com/CONUS/PrecipRate_00.00/20260809/MRMS_PrecipRate_00.00_20260809-165800.grib2.gz`,
retrieved 2026-08-09 17:05 UTC (upstream `Last-Modified` 17:00:5x, the ~2-3 min publication delay
WX1 measured):

- `mrms-conus-20260809-165800.grib2.gz` — 526,393 bytes, sha256 `cfdb8af34852e7d48ff5b618af79a0af2fd7877e161fdb3a134ee1acbfd3d4a0`.

## NOAA HRRR subhourly PRATE (CONUS forecast)

The 2026-08-09 15Z run's subhourly indexes, and the exact `PRATE` byte ranges the baker selects
from them. Objects are
`https://noaa-hrrr-bdp-pds.s3.amazonaws.com/hrrr.20260809/conus/hrrr.t15z.wrfsubhfFF.grib2`
(`.idx` for the indexes), retrieved 2026-08-09 17:05-17:15 UTC. The objects themselves are
210-221 MB and are never checked in — only the 30-38 KB messages the `.idx` selection resolves to.

Upstream object lengths (the `Content-Length` the range arithmetic is bounded by):
`f02` 210,757,046, `f03` 214,632,128, `f04` 220,555,508.

- `hrrr-conus-20260809T15-f01.idx` — 11,730 bytes, sha256 `133925d7aabfce4aaaf2a1329cd8286993b7aad6c26174c9ce413da611b9f592`.
- `hrrr-conus-20260809T15-f02.idx` — 11,842 bytes, sha256 `bd128ea3c893b881f14a0061e7fcbbe1c15ef08f4cc87a2f4c184369546c4573`.
- `hrrr-conus-20260809T15-f03.idx` — 11,962 bytes, sha256 `d9719faca61aed9cded5d0df3fcbfa173b59f45953b87a1369591344800513a8`.
- `hrrr-conus-20260809T15-f04.idx` — 11,963 bytes, sha256 `ff08f960b698f8cbe63c726d5b3f77664fc4d2c375a83aa32059e243fa2d9669`.

- `hrrr-conus-20260809T15-prate-t120.grib2` — 31,141 bytes, bytes `183664477-183695617` of that object, sha256 `82e64cdd75b5b0945e09f582c7aa60153d4813b5344ecd2f02f4cc573b7d42e0`.
- `hrrr-conus-20260809T15-prate-t135.grib2` — 30,706 bytes, bytes `25809346-25840051` of that object, sha256 `567eccabd6041d7a959df2bdb60a54f8aff2d32ae2edaec4cb80d088d7e3c6a5`.
- `hrrr-conus-20260809T15-prate-t150.grib2` — 32,162 bytes, bytes `79031140-79063301` of that object, sha256 `5e0425a0c3d0bff11bbf70ed23419d7ba49b0398a1d7e47419679c0aec838484`.
- `hrrr-conus-20260809T15-prate-t165.grib2` — 31,897 bytes, bytes `132718351-132750247` of that object, sha256 `173dc99794fb8f4905e4887adc693e8627c13629d7481a4a55f67dbe3cc923d7`.
- `hrrr-conus-20260809T15-prate-t180.grib2` — 33,430 bytes, bytes `186502886-186536315` of that object, sha256 `d96240594d77eaf1f072648c3a3bfcacc2f5ede8fb513f3e28a7681cfee023f3`.
- `hrrr-conus-20260809T15-prate-t195.grib2` — 34,300 bytes, bytes `26244769-26279068` of that object, sha256 `f1c18bdccfbad96358204188a3d81bd2d256dddfb75f15513b0d57dbf5dc824f`.
- `hrrr-conus-20260809T15-prate-t210.grib2` — 35,311 bytes, bytes `80983359-81018669` of that object, sha256 `3c0c6278c2f3dcd1a298319e1d258d1c5b368bca11e7f4267018dd282ce63848`.
- `hrrr-conus-20260809T15-prate-t225.grib2` — 36,177 bytes, bytes `136058399-136094575` of that object, sha256 `6db4d678576f3c8a540c3e22569e75e6b72e56387ef7d33d089496109a0a94af`.
- `hrrr-conus-20260809T15-prate-t240.grib2` — 37,839 bytes, bytes `191463451-191501289` of that object, sha256 `f0909ee821ed3275b6fa06eefb498ee700b6fa32c7f04892c7e04a1ae85dec11`.

## NOAA GFS instantaneous PRATE (worldwide floor)

The 2026-08-09 12Z run's first sixteen hourly indexes and the exact
`PRATE:surface:N hour fcst` messages. Objects are
`https://noaa-gfs-bdp-pds.s3.amazonaws.com/gfs.20260809/12/atmos/gfs.t12z.pgrb2.0p25.fFFF`
(`.idx` for the indexes), retrieved 2026-08-09 17:05-17:20 UTC. Each object is ~540 MB and is
never checked in. The PRATE messages below were retrieved from those same immutable objects on
2026-08-11 after the time-semantics review replaced de-accumulated APCP with point-valid rate.

Upstream object lengths: f001 537,540,348, f002 538,822,727, f003 539,798,514, f004 540,724,755,
f005 542,923,155, f006 544,451,780, f007 542,096,820, f008 543,890,390, f009 543,734,730,
f010 544,255,893, f011 544,322,108, f012 545,133,960, f013 541,397,261, f014 541,818,663,
f015 542,144,204, f016 546,445,777. The sixteen selected messages total 9,950,167
bytes, inside WX1's 15,500,000-byte per-run ceiling.

- `gfs-global-20260809T12-f001.idx` — 40,472 bytes, sha256 `ec6d58b4e473899badbe152fce6cebe5bdc2858113b2b1ad80d598804b91a1b7`.
- `gfs-global-20260809T12-f002.idx` — 40,472 bytes, sha256 `30ae1206f0ce9c4aef9acd69229145e63979d345491eba6614e71f79ac81bdb2`.
- `gfs-global-20260809T12-f003.idx` — 40,473 bytes, sha256 `7d3e233143ea1a433f684bfaa02668fe44b23ffca4c8c98999ac10f757593bb2`.
- `gfs-global-20260809T12-f004.idx` — 40,473 bytes, sha256 `e2faa4080cb1ce30aa3cc334eeea184013a2faedc9575707708df45a3040c378`.
- `gfs-global-20260809T12-f005.idx` — 40,472 bytes, sha256 `98342e7a2e98cba0e7e9c8cbecc74043280f245e924e051e71640c1ffb41914b`.
- `gfs-global-20260809T12-f006.idx` — 40,474 bytes, sha256 `02c56ee84d285ef4041237487b6663bb09d1d0d664dfc5381a01512eaf026c38`.
- `gfs-global-20260809T12-f007.idx` — 40,473 bytes, sha256 `28e543f672c939ff290a26269d0ee227e47fa41154381d0231d2bff7f8942ab2`.
- `gfs-global-20260809T12-f008.idx` — 40,474 bytes, sha256 `4a44309ccc0882bebfd75ce3aaad2c1699cb7c7525bf50353492becac44a2f7f`.
- `gfs-global-20260809T12-f009.idx` — 40,474 bytes, sha256 `e36809dc1f8876c8a2f2ea244794f510d8a18c410546576ec70f0d081f4c3819`.
- `gfs-global-20260809T12-f010.idx` — 41,218 bytes, sha256 `81e7acf2c1d99aad758adec34d8cfd2fc4269b78d4710992e8cc20b9b9c610ee`.
- `gfs-global-20260809T12-f011.idx` — 41,219 bytes, sha256 `af4887e80af1d7c49047a7fdb70c8237eabe3cc4cf9ea5397df2871372d775f0`.
- `gfs-global-20260809T12-f012.idx` — 41,219 bytes, sha256 `dbed2d57a68d07d9316db2408ea3458ef93a27466df51e6e9110752deb6c3747`.
- `gfs-global-20260809T12-f013.idx` — 41,258 bytes, sha256 `d6a5b821cfd9310b4a3a4cbf493bd992f1bee1e97a6cd43b122fcde0d83f954e`.
- `gfs-global-20260809T12-f014.idx` — 41,257 bytes, sha256 `27227747a769aec0bfa8e4da4edd61ebce5f4c93acb5b988e8c4e139a2825754`.
- `gfs-global-20260809T12-f015.idx` — 41,259 bytes, sha256 `df11f12232a448d766e417446380075f55dc483a565c709cf54455c4c3d6aa4b`.
- `gfs-global-20260809T12-f016.idx` — 41,260 bytes, sha256 `753a98a61cd672e375152abf50359e100ed0b5a7a0088bbb710fcea49098bd9f`.

- `gfs-global-20260809T12-prate-f001.grib2` — 685,198 bytes, bytes `425522639-426207836`, sha256 `9ff7233197fc256df2b25da3d6dfa9dbf73fc7837caa7ada8e82b18623cdfb3c`.
- `gfs-global-20260809T12-prate-f002.grib2` — 606,818 bytes, bytes `426107249-426714066`, sha256 `3cee0e89a81d62d9719757237f6422b71e81c13b1b8bcd2567c0193d6a44fa2a`.
- `gfs-global-20260809T12-prate-f003.grib2` — 605,410 bytes, bytes `426416776-427022185`, sha256 `a2741bc79cdc82b29d1784f1467c7ebb428d27c734ee866ca020690d9e97cab1`.
- `gfs-global-20260809T12-prate-f004.grib2` — 602,199 bytes, bytes `426700119-427302317`, sha256 `104084b692f54e585bd1364f5ca2aa4c151265156be07c9e70a13bb507a84756`.
- `gfs-global-20260809T12-prate-f005.grib2` — 600,287 bytes, bytes `428037470-428637756`, sha256 `eaa3e6a0f6b2f2fa89c8370c86d81ab7485adb5b27b7dcc829ad329be5a748fe`.
- `gfs-global-20260809T12-prate-f006.grib2` — 684,759 bytes, bytes `428864376-429549134`, sha256 `84f62ec4cee80728f2d4f45b8ce84624c6c06114a354f327f3b44927cc32c430`.
- `gfs-global-20260809T12-prate-f007.grib2` — 681,644 bytes, bytes `429766627-430448270`, sha256 `ddc0c988289bdfb5a3300bf3016ff07dc1286090dd8e77988d000932f3930a80`.
- `gfs-global-20260809T12-prate-f008.grib2` — 591,454 bytes, bytes `430793964-431385417`, sha256 `0c6fe17b52dd71874b2d336cd67aba22bdefb3d1fe487918625b3b3f1871590d`.
- `gfs-global-20260809T12-prate-f009.grib2` — 584,044 bytes, bytes `430037496-430621539`, sha256 `5dccf86a479920df946e2feb5d3e15278e47131f5a87d38f6633c359047a1c34`.
- `gfs-global-20260809T12-prate-f010.grib2` — 592,549 bytes, bytes `430052839-430645387`, sha256 `e4dc5afde6e4b208fc4c48f1809886848fd930c52ce31a8d6c9b57f67dff9b22`.
- `gfs-global-20260809T12-prate-f011.grib2` — 584,120 bytes, bytes `429709935-430294054`, sha256 `f61cc885fa07fb8dd1b4b104f755c720c3137e4efd9026e2424ca6943b06a7e3`.
- `gfs-global-20260809T12-prate-f012.grib2` — 673,567 bytes, bytes `429803547-430477113`, sha256 `2ba059a876e88b72e66698002d6b4f5563224f59434a21f89a1eab5b8acbd344`.
- `gfs-global-20260809T12-prate-f013.grib2` — 591,602 bytes, bytes `428939943-429531544`, sha256 `8af7fef0cfb4231e6dca2711c0d383a8ebdd7a7f9777e6ad2385ca07c6202bfa`.
- `gfs-global-20260809T12-prate-f014.grib2` — 586,737 bytes, bytes `428485546-429072282`, sha256 `bc705ab9f059f1bb4168555ca7ddcd346310f4187c6f37b9862d9a4ac4b25b77`.
- `gfs-global-20260809T12-prate-f015.grib2` — 597,010 bytes, bytes `428388965-428985974`, sha256 `45e1dfcd51813fc881a8db3d52c5706308a92b047afc133e4bd3f787224308db`.
- `gfs-global-20260809T12-prate-f016.grib2` — 682,769 bytes, bytes `430855401-431538169`, sha256 `8f7654210b08ee5b99b58ff2c581d96532581b9daa4f1d9919cb4f3e562ff9e9`.

Corrupt-upstream negatives are derived in-test by truncating/flipping/splicing these bytes; no
corrupt fixture is checked in.

## EUMETNET OPERA CIRRUS / NIMBUS (Europe radar)

Two composites valid at 2026-08-10T00:00:00Z, from the live 24-hour bucket
`https://s3.waw3-1.cloudferro.com/openradar-24h/2026/08/10/OPERA/COMP/`, retrieved 2026-08-10
21:33 UTC. Licence CC BY 4.0, as each object's own `GDAL_METADATA` states.

Upstream objects (**not** checked in — 6.5 MB between them, and the baker reads the whole file):

- `OPERA@20260810T0000@0@DBZH.tiff` — 3,563,217 bytes,
  sha256 `5a02635e8af7731bd9921c16830b82eef3823e9f8f703a5c5379b6a1f3771c6c`,
  upstream `Last-Modified` Mon, 10 Aug 2026 00:04:10 GMT (the measured 4.1-minute lag),
  ETag `"8ed7d9895c5e285c1fe97578d892ed0d"`.
- `OPERA@20260810T0000@0@RATE.tiff` — 2,983,924 bytes,
  sha256 `479a70812e3222bdac5c91b5cfced38d063151c81c74d1530934d60e762ea58c`,
  upstream `Last-Modified` Mon, 10 Aug 2026 00:10:03 GMT, ETag `"84e043de254b7914fb6e14ce07e771c5"`.

What is checked in is a **tile crop** of each: a rectangular block of the upstream file's own
512 x 512 deflate streams, copied verbatim, re-wrapped in a classic TIFF whose tags are the
upstream's IFD 0 except for `ImageWidth`/`ImageLength`, the tile tables and a `ModelTiepoint`
shifted to the block's own upper-left corner. Not a resample and not a re-encode: every sample a
test reads is a byte EUMETNET published. This is the block-addressable analogue of the HRRR/GFS
entries above, which check in exact byte ranges of objects too large to store — the payload here
is addressed by tile rather than by offset because that is how a COG is laid out.

- `opera-cirrus-20260810T0000-dbzh-crop.tiff` — 78,283 bytes,
  sha256 `37029d14c5bea6072797f19941e856853753e9f074ed48f76ee2960c6795cdbb`.
  Tiles (rows 5-6, cols 3-4) of the 3,800 x 4,400 composite: 1,024 x 1,024 native 1 km cells over
  the Alps, northern Italy and the Balkans. Grid corner (1536000, -2560000); tiepoint
  (1535499.9997285667, -2559500.000087613), i.e. that corner less half a pixel, exactly as the
  upstream object states its own.
  Content mix: 2.64 % no-coverage, 93.87 % undetect, 3.49 % finite, -21.0 to 55.5 dBZ.
- `opera-nimbus-20260810T0000-rate-crop.tiff` — 170,558 bytes,
  sha256 `951cb8bd3737e4ab48d949125cb69ee5d9b68abb0e89fe5666de00cb05acf923`.
  Tiles (rows 3-4, cols 1-2) of the 1,900 x 2,200 composite: 1,024 x 664 native 2 km cells over
  the central Mediterranean, deliberately including the composite's **partial southern tile
  row**, so the decoder's edge-padding handling is exercised on real bytes. Grid corner
  (1024000, -3072000); tiepoint (1022999.9997285667, -3071000.000087613). Content mix: 51.98 %
  no-coverage, 45.91 % undetect, 2.11 % finite, 0.02 to 577.9 mm/h.

Both crops keep the upstream `GDAL_METADATA`, `GDAL_NODATA` and GeoTIFF keys verbatim, so
`tests/opera.rs` verifies the whole pinned source contract — `prodname`, band `DESCRIPTION`,
`undetect`, `zr_a`/`zr_b`, the composite's own `date`/`time`, the projection parameters and the
nodata sentinel — against real bytes. Negatives are patched in-test with equal-length
replacements, so no tag offset moves and no corrupt fixture is checked in.

The regeneration recipe is: fetch the upstream object, take tiles `[r0..=r1] x [c0..=c1]` from
IFD 0, and emit a TIFF whose IFD is IFD 0's tags with the four listed substitutions. Note that
the `openradar-archive` bucket (which reaches back to 2012) publishes **only** the ODIM HDF5
twins, so an archived crop would be a different format; the bakery only ever reads the live
24-hour bucket, which carries both.
