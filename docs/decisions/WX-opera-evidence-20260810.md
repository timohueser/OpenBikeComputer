# OPERA calibration, quality and priority evidence — 2026-08-10

This is the evidence gate for changing CIRRUS's column-maximum calibration, applying OPERA's
quality plane, or reordering CIRRUS/NIMBUS/DWD. It records what one complete day supports and,
equally importantly, what it does not.

## Inputs and alignment

The pass used the 24 hourly instants `00:00` through `23:00 UTC` on 2026-08-10:

- OPERA CIRRUS `OPERA@20260810THH00@0@DBZH.tiff`, 1 km column-maximum reflectivity;
- OPERA NIMBUS `OPERA@20260810THH00@0@RATE.tiff`, 2 km instantaneous near-surface rate;
- DWD `composite_rv_20260810_HH00.tar`, lead-000 1 km five-minute accumulation converted to
  mm/h with the member's own gain/offset.

The objects were retrieved from the production endpoints on 2026-08-11. OPERA's live COGs expire,
but their immutable key schema and the archive's ODIM-HDF5 twins allow the pass to be rebuilt. DWD
keeps the timestamped RV tars beside `composite_rv_LATEST.tar`.

CIRRUS and NIMBUS share an exact 2:1 LAEA registration. Four finite CIRRUS rates were averaged
onto each NIMBUS cell; a comparison cell had to contain a positive finite echo in both products.
DWD pixels were transformed from the pinned DWD stereographic grid onto the same OPERA LAEA grid
with nearest-neighbour sampling. DWD RV is an independent national radar product, not literal
ground truth; its different five-minute window makes this a consistency check, not an accuracy
certificate.

## Calibration result

Across 3,684,756 positive CIRRUS/NIMBUS overlap cells, the uncorrected Marshall-Palmer CIRRUS rate
divided by NIMBUS had:

| statistic | ratio |
| --- | ---: |
| pooled median | 1.985 |
| pooled p25 / p75 | 1.129 / 3.894 |
| minimum / median / maximum hourly median | 1.616 / 2.036 / 2.218 |

The original `2.2` was therefore not a one-frame accident, but neither is the relationship a
precise scalar. Splitting cells by the maximum CIRRUS reflectivity in their 2 x 2 block produced a
median ratio of 1.775 below 30 dBZ (2,898,242 cells) and 3.068 at or above 30 dBZ (786,514 cells).
A hard two-regime divisor would make rate fall as reflectivity crosses 30 dBZ, so it is not a valid
mapping. A monotone fitted relation needs several weather regimes and gauge truth before replacing
the physically understandable scalar.

Against DWD lead-000 where both predicted and DWD rates were positive, `/2.2` was slightly better
than `/2.0` over the 24 hourly frames:

| correction | median frame median predicted/DWD | median absolute log2 error | median share within 2x |
| --- | ---: | ---: | ---: |
| CIRRUS / 2.2 | 1.550 | 0.916 | 53.6 % |
| CIRRUS / 2.0 | 1.705 | 0.972 | 51.3 % |

Decision: keep `MAX_TO_SURFACE_RATIO = 2.2`. The full-day OPERA-only median argues for roughly 2.0,
while the independent DWD comparison weakly prefers 2.2; that is not enough evidence to retune a
continental rain-rate field.

## Quality-plane result

`pl.imgw.quality.qi_total` is normalized to `0..=1`, but its population is not interchangeable
between the two products:

| product | finite echo cells | echo QI below 0.6 | covered-dry cells with missing QI |
| --- | ---: | ---: | ---: |
| CIRRUS | 27,297,271 | 92.71 % | 174,583,030 / 174,583,030 |
| NIMBUS | 3,836,090 | 5.81 % | 0 / 46,798,126 |

A generic `QI < 0.6 => no-data` rule would erase most CIRRUS echo, while treating missing QI as
bad would erase every CIRRUS dry observation. The quality plane is therefore decoded and validated
as a source-contract signal, with no threshold applied to map cells. Any future filter must be
product-specific and demonstrate improved event skill, including dry/wet classification, before
it changes production output.

## Source-priority result

At a 0.1 mm/h wet threshold over the common DWD-covered domain, accumulated across all 24 frames:

| source | CSI | probability of detection | false-alarm ratio |
| --- | ---: | ---: | ---: |
| corrected CIRRUS | 0.656 | 0.978 | 0.334 |
| NIMBUS | 0.672 | 0.708 | 0.071 |

The trade is real: NIMBUS is conservative and physically nearer the surface; CIRRUS detects much
more light echo and is twice as fine and three times as frequent. One day does not justify hiding
either strength. Keep DWD above both OPERA composites, and keep CIRRUS above NIMBUS as the fresh
fine field with NIMBUS filling its holes. Reordering the OPERA pair should require multi-event
scores at the actual bakery publication phase, where CIRRUS's five-minute cadence and NIMBUS's
ten-minute publication delay are represented.
