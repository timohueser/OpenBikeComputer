# OBCM — OpenStreetMap → compact binary maps

OBCM ("OSM Binary Compact Map") is a pipeline and file format for turning
OpenStreetMap extracts into small, LOD-tiered binary maps that an embedded
device can render directly — the target being an nRF5340 driving a
LS021B7DD02 memory LCD.

```
  .osm.pbf  ──►  obcm_pack  ──►  *.obcm (v3)  ──►  Rust viewer / firmware
 (OSM data)     (this repo)     (binary map)      (viewer-rs/, shared render)
```

The `.obcm` format is a self-contained LOD pyramid: a global style table plus,
per zoom tier, a quadtree of geometry chunks. Readers are fully table-driven —
nothing depends on specific style-ID values. The on-disk layout is specified in
[`OBCM_Spec.md`](OBCM_Spec.md).

## Repository layout

| Path | What it is |
| :-- | :-- |
| `obcm/` | Python packing library: `config`, `ingest` (OSM → features), `quadtree`, `serialize` (→ binary), `land_ingest` (coastline/land polygons) |
| `obcm_pack.py` | CLI that runs the full pack pipeline |
| `config.json` | Feature selection + styling (which OSM tags to keep, colors, z-order, per-LOD detail). Style IDs are auto-assigned at build time — see below. |
| `webapp/` | FastAPI web builder: pick regions on a map, edit styles, build an `.obcm` in the browser |
| `viewer-rs/` | Rust workspace — `no_std` reader + renderer (shared with firmware) and an SDL2 desktop simulator. See [`viewer-rs/README.md`](viewer-rs/README.md). |
| `OBCM_Spec.md` | The v3 binary format specification |
| `tests/` | Python unit tests (`pytest`) |

## Setup

Python 3.13, dependencies in `requirements.txt`:

```sh
python -m venv .venv
.venv/bin/python -m pip install -r requirements.txt
# (this project's .venv is uv-managed: `uv pip install -r requirements.txt`)
```

Packing also needs the [`osmium`](https://osmcode.org/osmium-tool/) CLI on your
`PATH` (only when merging multiple `.pbf` inputs).

## Packing a map

Download an extract (e.g. from [Geofabrik](https://download.geofabrik.de/)),
then:

```sh
.venv/bin/python obcm_pack.py region.osm.pbf config.json region.obcm
```

- Multiple `.pbf` inputs are merged (via `osmium merge`) before ingest.
- `--chunk-size N` sets the quadtree chunk payload size (default 4096).
- LOD tiers and feature styling come from `config.json`; coastline/land
  polygons are generated automatically when `natural.land` is configured.

### Web builder

For an interactive flow — select regions, tweak styles, watch build progress:

```sh
.venv/bin/python -m webapp        # http://localhost:8000
```

- `config.json` is the read-only **factory default**. Edits made in the builder
  are saved automatically to `user_config.json` (gitignored) and persist between
  sessions; **Restore defaults** discards them.
- Styles can be shared independently of any `.obcm`: **Export stylesheet** writes
  the current config as a `.json`, **Import stylesheet** loads one back in.
- Feature/category fields autocomplete from a curated catalog of common OSM tag
  keys/values (`webapp/static/osm_catalog.json`); any freeform tag is still
  accepted.

## Viewing a map

The Rust simulator renders `.obcm` files with the same code path as the
firmware:

```sh
cd viewer-rs && cargo build --release
./target/release/obcm-sim ../region.obcm
```

See [`viewer-rs/README.md`](viewer-rs/README.md) for build requirements (SDL2)
and options.

## Style IDs

Style IDs are a purely internal `uint8` reference into each file's style table;
no reader depends on a specific value, only on uniqueness. You therefore **don't
author them** — `obcm/config.py` assigns them deterministically (1..N, document
order) at load time, so collisions are impossible by construction. `config.json`
carries no `id` fields and the web builder has no ID column.

## Testing

```sh
.venv/bin/python -m pytest tests/
```

> The Python reader/viewer was removed in favor of the Rust viewer; a fresh
> Python test suite and a Python-pack → Rust-read cross-language test are the
> next step.
