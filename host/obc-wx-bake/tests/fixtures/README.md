# obc-wx-bake fixtures

Real, unmodified upstream objects (and exact upstream byte ranges) captured on 2026-08-09 UTC.
They drive the deterministic fixture cycles in `tests/cycle.rs` and `tests/us_gfs_cycle.rs`: same
fixtures ⇒ byte-identical published tree, and every corruption of them must publish nothing.

Provenance discipline: every entry below records the exact retrieval URL, the byte range where
one was used, the length and the SHA-256 of the checked-in bytes. Terms are DWD Open Data
(CC BY 4.0) for the German sources and NOAA Open Data Dissemination (public-use U.S. government
data, no endorsement implied) for the NOAA ones; the license record lives in
[`docs/decisions/WX1-weather-source-contracts.md`](../../../../docs/decisions/WX1-weather-source-contracts.md).

Total checked-in fixture bytes: 14,902,481.

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

## NOAA GFS APCP (worldwide floor)

The 2026-08-09 12Z run's first twelve hourly indexes and the exact `APCP:surface:0-N hour acc
fcst` spans. Objects are
`https://noaa-gfs-bdp-pds.s3.amazonaws.com/gfs.20260809/12/atmos/gfs.t12z.pgrb2.0p25.fFFF`
(`.idx` for the indexes), retrieved 2026-08-09 17:05-17:20 UTC. Each object is ~540 MB and is
never checked in. Leads 1-6 resolve to a two-message span (NOAA advertises the record twice and
both copies must decode identically); leads 7-12 resolve to a single message.

Upstream object lengths: f001 537,540,348, f002 538,822,727, f003 539,798,514, f004 540,724,755,
f005 542,923,155, f006 544,451,780, f007 542,096,820, f008 543,890,390, f009 543,734,730,
f010 544,255,893, f011 544,322,108, f012 545,133,960. The twelve selected spans total 6,415,845
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

- `gfs-global-20260809T12-apcp-f001.grib2` — 488,920 bytes, bytes `427603385-428092304` of that object, sha256 `35fce3acac40fd09b314f7fe6210c33cb8541c54e736189e45a10a39dd454ebb`.
- `gfs-global-20260809T12-apcp-f002.grib2` — 587,546 bytes, bytes `428091880-428679425` of that object, sha256 `aba63ddc21e014b083aad91a87eaa532a4c85ad1022d5d8e43808bffb237df05`.
- `gfs-global-20260809T12-apcp-f003.grib2` — 645,428 bytes, bytes `428475805-429121232` of that object, sha256 `bfce292b647a5afd04ad94ef491837743b070d550d2b49fcaff488e7be31a64a`.
- `gfs-global-20260809T12-apcp-f004.grib2` — 688,958 bytes, bytes `428752482-429441439` of that object, sha256 `eec9efe8e1e5dd237abcf17c968a2adf0f8092748ee782fce4ed6a2b5ed86d0a`.
- `gfs-global-20260809T12-apcp-f005.grib2` — 723,720 bytes, bytes `430080077-430803796` of that object, sha256 `2b2688e6128b3f284c1b54f91584d17ebf6d9beb6d9a8661c333cf4e76bb6d3e`.
- `gfs-global-20260809T12-apcp-f006.grib2` — 754,116 bytes, bytes `431023684-431777799` of that object, sha256 `398b845d2c08df6129b8bfada11c9642eef21118cebf5694a23218084a7e1927`.
- `gfs-global-20260809T12-apcp-f007.grib2` — 393,493 bytes, bytes `432070312-432463804` of that object, sha256 `7bf9eab443b44be33eb0b9d434d05991218065c1fd7f0dd46c36a726411c1add`.
- `gfs-global-20260809T12-apcp-f008.grib2` — 405,659 bytes, bytes `433033986-433439644` of that object, sha256 `3db99fe1e87142e12d06bc96ace6b77b363b4944a6b26ebfccbf4549d087ae49`.
- `gfs-global-20260809T12-apcp-f009.grib2` — 416,142 bytes, bytes `432288308-432704449` of that object, sha256 `a44ff265140ace460a64281c969aee6ddbe0cc5b9fb39f9a2fe27bdef1c36c41`.
- `gfs-global-20260809T12-apcp-f010.grib2` — 426,897 bytes, bytes `432328102-432754998` of that object, sha256 `420707baa4f956e9271bb22fc3d702206d86572377a47668d8b43989f2d46927`.
- `gfs-global-20260809T12-apcp-f011.grib2` — 436,221 bytes, bytes `431989179-432425399` of that object, sha256 `5b6f2f1c50b0f5e4881687e7e386cc5087bee40852c1cfcb168ea368f0b1c894`.
- `gfs-global-20260809T12-apcp-f012.grib2` — 448,745 bytes, bytes `432276114-432724858` of that object, sha256 `821310bd859a37fadaa9ba8c62474101591d9c3fbb64af00575d9d705eef1472`.

Corrupt-upstream negatives are derived in-test by truncating/flipping/splicing these bytes; no
corrupt fixture is checked in.
