# Weather source decisions

Status: **active source set; production behavior lives in `host/obc-wx-bake`**

WX1 established which upstream precipitation products were acceptable and which boundaries belong
on the server. The original client-selected tier ladder is retired. The bakery now resamples every
source onto one canonical 0.01° lattice and resolves overlaps per cell with the ordered
`source::MOSAIC_PRIORITY` table. Clients see one provider-neutral dataset.

This record keeps the decisions and evidence that are not obvious from code. Adapter endpoints,
grid templates, sentinels and decoding limits belong beside their fail-closed implementations under
`host/obc-wx-bake/src/source/` and `src/grib.rs`.

## Frozen boundaries

- Provider archives, GRIB/HDF5/TIFF decoding, reprojection and precipitation normalization run in
  the stateless host bakery, never in Swift or device firmware.
- Adapters return provider-neutral grids. They do not publish directly and downstream code does not
  branch on provider encodings.
- A complete cycle validates before atomic publication. A failed or incomplete source falls through
  to the next eligible source; missing or expired data is never painted as dry weather.
- Source rank and frame eligibility are different decisions. An observation may paint the anchor
  frame only; models and derived nowcasts must carry genuine forward-valid frames.
- MET Norway Locationforecast remains the explicit phone-side point-forecast exception. It is not a
  gridded bakery source.
- NASA IMERG Early remains a v1 **NO-GO**: its latency and half-hourly 0.1° estimate do not add a
  trustworthy observation layer over the global GFS floor.

## Current mosaic priority

The code table is authoritative. Ordered best first:

| Source | Role | Decision |
| --- | --- | --- |
| DWD RV | German 1 km radar nowcast | National native nowcast wins in Germany |
| NOAA MRMS | CONUS 1 km radar observation | Anchor observation only |
| OPERA CIRRUS | Pan-European 1 km reflectivity | Finest European observation outside national radar |
| OPERA NIMBUS | Pan-European 2 km rain rate | Native near-surface fill where CIRRUS has no answer |
| MRMS-derived nowcast | Advected CONUS radar | Forward radar estimate below observations, above models |
| CIRRUS-derived nowcast | Advected European radar | Same rule over the CIRRUS footprint |
| NOAA HRRR | CONUS 3 km model | Regional forward forecast and radar-hole fill |
| DWD ICON-EU | European 6.5 km model | Regional forward forecast and radar-hole fill |
| NOAA GFS | Global 0.25° model | Worldwide floor; last eligible source |

The ordering rule is: national radar before pan-regional radar, observations before derived
nowcasts, derived nowcasts before models, regional models before the global floor. DWD RV already
publishes real forward members and is therefore not advected again.

Rank never repeats a current observation into future frames. If a regional forecast is absent, the
cell falls through to a coarser eligible forecast rather than relabeling an old radar image.

## OPERA decisions

CIRRUS column-maximum reflectivity is converted with Marshall–Palmer and divided by
`MAX_TO_SURFACE_RATIO = 2.2`. NIMBUS supplies native near-surface rate. The quality plane is decoded
and validated but not thresholded generically: one day's population showed that a common cutoff
would erase most CIRRUS echo and all CIRRUS covered-dry cells.

The 24-hour calibration, DWD comparison and priority scores are preserved in
[`WX-opera-evidence-20260810.md`](WX-opera-evidence-20260810.md). Retuning the ratio, filtering by
quality or reordering CIRRUS/NIMBUS requires multi-event evidence at the actual publication cadence.

## Licensing and attribution

Every source that may paint a generation appears in the manifest attribution list in priority
order. Required notices include:

- DWD: DWD source, modification/quantization notice and applicable open-data terms;
- NOAA: NOAA/NCEP product, modification notice and no-endorsement language;
- OPERA: EUMETNET OPERA product, CC BY 4.0 and the CIRRUS conversion/calibration notice;
- derived nowcasts: the parent's attribution plus the OpenBikeComputer advection notice;
- MET Norway: the phone-side `Data from MET Norway` notice.

Exact captured-object URLs, hashes and terms are fixture provenance, not prose here. See
[`fixtures/catalog.toml`](../../fixtures/catalog.toml), [`fixtures/README.md`](../../fixtures/README.md)
and `host/obc-wx-bake/tests/fixtures/README.md`.

## Reproduction

Run the checked-in adapter, canonical-lattice and publication tests:

```sh
obc test -p obc-wx-bake
```

External historical packages are opt-in:

```sh
obc test fixtures -p obc-wx-bake
```

A live cycle discovers current upstream runs and fails closed on contract drift:

```sh
cargo run --release -p obc-wx-bake -- cycle --store /tmp/obc-weather-evidence
```

The production source table, frame-eligibility rules, canonical resampling and manifest tests are
the continuing evidence. Operations, retention and outage behavior live in
[`ops/weather/RUNBOOK.md`](../../ops/weather/RUNBOOK.md); client-facing object semantics live in
[`OBCG_Spec.md`](../../specs/OBCG_Spec.md) and [`OBCW_Spec.md`](../../specs/OBCW_Spec.md).
