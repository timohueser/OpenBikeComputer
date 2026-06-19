# OBCM — OpenStreetMap → compact binary maps

OBCM ("OSM Binary Compact Map") is a pipeline and file format for turning
OpenStreetMap extracts into small, LOD-tiered binary maps that an embedded
device can render directly — the target being an nRF54L driving a
LS021B7DD02 memory LCD.

```
  .osm.pbf  ──►   obc-pack   ──►  *.obcm (v5)  ──►   OBC firmware
 (OSM data)     (Rust packer)     (binary map)     (firmware/, shared render)
```

The `.obcm` format is a self-contained LOD pyramid: a global style table plus,
per zoom tier, a quadtree of geometry chunks. Readers are fully table-driven —
nothing depends on specific style-ID values. The on-disk layout is specified in
[`OBCM_Spec.md`](OBCM_Spec.md).

## Repository layout

| Path | What it is |
| :-- | :-- |
| `firmware/obc-pack/` | The map packer (**Rust**): OSM `.osm.pbf` → `.obcm` — ingest, multipolygon assembly, land generation, quadtree, serialize |
| `packer/config.json` | Feature selection + styling (which OSM tags to keep, colors, z-order, per-LOD detail). Style IDs are auto-assigned at build time — see below |
| `packer/web_builder/` | FastAPI web builder: pick regions on a map, edit styles, build an `.obcm` in the browser (shells out to `obc-pack`) |
| `firmware/` | Rust workspace — the OBC firmware, a desktop simulator, and the `obc-pack` packer, sharing one `no_std` reader + renderer. See [`firmware/README.md`](firmware/README.md) |
| `OBCM_Spec.md` | The binary map-format specification (v5) |
| `OBCR_Spec.md` | The on-device route-format specification |

> The packer was originally a Python pipeline (`packer/pack.py` + `packer/obcm/`);
> it has been ported to Rust (`firmware/obc-pack`) and the Python pipeline removed.
> The design notes for that port live in `packer/docs/`.

## Setup

The packer is a Rust binary; build it with a stable Rust toolchain. It links
system **GEOS** (`brew install geos`) and uses the
[`osmium`](https://osmcode.org/osmium-tool/) CLI on your `PATH` when merging
multiple `.pbf` inputs.

```sh
cargo build --release -p obc-pack --manifest-path firmware/Cargo.toml
```

The web builder (optional, below) is a small Python app — Python 3.13 with the
deps in `packer/requirements.txt`:

```sh
python -m venv .venv
.venv/bin/python -m pip install -r packer/requirements.txt
# (this project's .venv is uv-managed: `uv pip install -r packer/requirements.txt`)
```

## Packing a map

Download an extract (e.g. from [Geofabrik](https://download.geofabrik.de/)),
then:

```sh
firmware/target/release/obc-pack region.osm.pbf packer/config.json region.obcm
```

- Multiple `.pbf` inputs are merged (via `osmium merge`) before ingest.
- `--chunk-size N` sets the quadtree chunk payload size (default 4096);
  `--no-land` skips land generation.
- LOD tiers and feature styling come from `config.json`; coastline/land polygons
  are generated automatically when `natural.land` is configured (the land-polygon
  dataset is downloaded and cached under `~/.cache/obcm/` on first use).

### Web builder

For an interactive flow — select regions, tweak styles, watch build progress:

```sh
.venv/bin/python -m packer.web_builder        # http://localhost:8000
```

It drives the `obc-pack` binary you built in Setup (override its location with
`OBC_PACK_BIN`). Other notes:

- `config.json` is the read-only **factory default**. Edits made in the builder
  are saved automatically to `user_config.json` (gitignored) and persist between
  sessions; **Restore defaults** discards them.
- Styles can be shared independently of any `.obcm`: **Export stylesheet** writes
  the current config as a `.json`, **Import stylesheet** loads one back in.
- Feature/category fields autocomplete from a curated catalog of common OSM tag
  keys/values (`packer/web_builder/static/osm_catalog.json`); any freeform tag is
  still accepted.

## Viewing a map

The Rust simulator renders `.obcm` files with the same code path as the
firmware:

```sh
cd firmware && cargo build --release
./target/release/obc-sim ../region.obcm
```

The GUI host is pure Rust (eframe/egui) — no SDL or system libraries to install.
See [`firmware/README.md`](firmware/README.md) for options.

## Style IDs

Style IDs are a purely internal `uint8` reference into each file's style table;
no reader depends on a specific value, only on uniqueness. You therefore **don't
author them** — the packer assigns them deterministically (1..N, document order)
at config-load time, so collisions are impossible by construction. `config.json`
carries no `id` fields and the web builder has no ID column.

## Testing

```sh
cargo test -p obc-pack --manifest-path firmware/Cargo.toml
```

The `obc-pack` tests use fixtures under `packer/tests/corpus/` — the committed
`tiny/tiny.osm` plus `config.json`. Regenerate the binary fixtures with
`packer/tests/corpus/build_corpus.sh` (needs `osmium`).
