# obc-desktop — the OpenBikeComputer desktop app

The web builder, shipped as an app: the same Svelte frontend in a Tauri v2 window,
with **`obc-pack` linked in as a library** instead of a Python server in front of a
subprocess (issue #906, phase D of #894).

What the concepts are and why the tiers split the way they do lives in the docs
site ([packer & routing](../../docs/content/software/packer-routing.md)). This file
is how you build and run it.

## What you need installed

Deliberately short, and deliberately **not GEOS** (see below):

| | |
|---|---|
| Rust | stable, via `rustup` |
| Node | 22+, for the frontend this crate embeds |
| CMake + a C++ compiler | to build the vendored GEOS — Xcode CLT / MSVC / gcc |
| Linux only | WebKitGTK for `wry`: `libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev libgtk-3-dev librsvg2-dev` |

Windows and macOS need nothing else — no vcpkg, no Homebrew, no WebView2 install
on Windows 10/11 (it ships with the OS).

## Build and run

The Rust side embeds the built frontend, so the frontend is built first:

```sh
cd packer/web_builder/frontend
npm ci
npm run build:wasm        # the GPX↔OBCR bridge the app imports
npm run build:desktop     # → dist/desktop, which tauri.conf.json embeds

cd ../../../firmware/obc-desktop
cargo run --release       # opens the window
```

`cargo run` is the whole story — there is no `tauri` CLI in this repo. That works
because `custom-protocol` is a **default feature** here, which is what tells Tauri
to embed `dist/desktop` rather than point the window at a dev server. The upstream
templates leave that feature off and have their CLI add it, which produces a blank
white window for anyone running plain `cargo`; this crate does not.

For a live-reload loop, opt out of it and run Vite instead:

```sh
npm --prefix ../../packer/web_builder/frontend run dev -- --mode desktop
cargo run --no-default-features --features vendored-geos   # follows :5173
```

The two default features are orthogonal on purpose, so neither flag ever means two
things at once:

| you want | pass |
|---|---|
| the app | *(nothing — both defaults)* |
| live reload against Vite | `--no-default-features --features vendored-geos` |
| your system GEOS, for a faster clean build | `--no-default-features --features custom-protocol` |

## GEOS is inside the binary (#907)

`obc-pack` links libGEOS — a C++ library — for [area
assembly](../../docs/content/software/packer-routing.md), the simplify and the
quadtree clip. In the `firmware/` workspace that is the *system* library, which is
right there: a developer has `brew install geos` and a from-source C++ build on
every clean checkout would be two and a half minutes nobody asked for.

This crate is the artifact a **user** installs, so it cannot ask that of them —
and on Windows "install GEOS 3.14" is barely a sentence. The `vendored-geos`
feature (on by default) pulls `geos-src`, which carries the GEOS 3.14.1 sources
inside the crate, builds them with CMake, and links the result **statically**:

```console
$ otool -L target/release/obc-desktop | grep -ci geos
0
```

Nothing to install, nothing to ship beside the binary, no `.dll`/`.dylib`/`.so`
search path at run time. What it costs, measured on an M-series Mac:

| | clean `cargo build --release` | binary |
|---|---|---|
| system GEOS (`--no-default-features --features custom-protocol`) | 2m 52s | 11.9 MB |
| vendored (default) | 5m 25s | 13.0 MB |

Only the *first* build in a target directory pays it — cargo caches the CMake
output like any other build-script product, so an incremental rebuild is
unchanged. If you have system GEOS ≥ 3.14 and want the faster clean build, the
opt-out is the same flag as the Vite loop above:

```sh
cargo build --release --no-default-features --features custom-protocol
```

**How the feature reaches `obc-pack`** is worth knowing before you edit either
manifest. This crate lists `geos` as a dependency it never calls; that entry
exists only to add `static` to the feature set of the *same* `geos` package
`obc-pack` depends on, which cargo unifies across the build graph. There is no
matching feature on `obc-pack` on purpose — `firmware/` is a separate cargo root,
so its `--all-features` clippy and test runs are unaffected and keep using the
system library. Keep the two `geos` version requirements identical; `tests/
geos_smoke.rs` fails if they drift.

That test is the whole guarantee. A successful *link* proves very little about a
C++ library reached over a C ABI, so it packs `packer/tests/corpus/data/
tiny.osm.pbf` through the linked GEOS and pins the SHA-256 of the result — one
assertion that covers "GEOS ran", "GEOS was the 3.14 we vendored", and "every
platform produces the same bytes". CI runs it on Linux, macOS and Windows MSVC,
on runners with **no** system GEOS at all, which is what makes a green matrix mean
something: if vendoring stopped working, `geos-sys` would find no library anywhere
and every leg would go red rather than quietly linking someone else's.

> **Licensing.** GEOS is LGPL-2.1 and is now *inside* the artifact rather than
> beside it. Compatible with this project's GPL-3 (LGPL-2.1 §3), but a distributed
> build has to carry the licence text and an offer of source — D3 (#908) owns the
> installers and therefore owns that.

## Checks

Standalone cargo root, like `obc-fw-nrf54l` and `obc-boot` — **not** a member of
the `firmware/` workspace, so it needs its own invocations (and gets its own CI
job):

```sh
cargo fmt                 # the third + fourth step of CLAUDE.md's fmt dance
cargo clippy --release --all-targets -- -D warnings
cargo test --release -- --nocapture     # includes the GEOS smoke test above
```

`--release` on both is what CI does, and for a reason worth copying locally:
cargo keys build-script output on the profile, so a debug clippy followed by a
release build compiles the whole of GEOS **twice**. One profile, one GEOS.

Two tests are `#[ignore]`d because they pack a real region and therefore need a
warm cache — a `.pbf` under `~/.cache/obcm/pbf`, the Geofabrik index, and the
land-polygon dataset. They are the ones that answer #906's acceptance criteria:

```sh
# the app's map and the CLI's map, byte for byte, with digests
cargo build --release -p obc-pack --manifest-path ../Cargo.toml
cargo test --release -- --ignored --nocapture

# cancellation on a region big enough for it to matter
OBC_TEST_REGION=freiburg-regbez cargo test --release cancelling -- --ignored --nocapture
# OBC_TEST_TRACE=1 additionally prints every event with a timestamp, which is how
# you find out *where* a cancel is stuck.
```

Run them in `--release`. In a debug build the tail after a cancel is dominated by
teardown — freeing a country's worth of ingested geometry with unoptimized drop
glue takes tens of seconds — and the ratio stops meaning anything.

## Where it puts things

| | |
|---|---|
| Built maps | `~/Documents/OpenBikeComputer/` |
| `.pbf` extracts | `~/.cache/obcm/pbf/` |
| Land-polygon dataset | `~/.cache/obcm/land/` |
| Geofabrik index | `~/.cache/obcm/geofabrik/` |

The caches are the **shared** ones (`OBCM_CACHE_DIR` overrides the root, exactly as
`packer/web_builder/paths.py` reads it), not a per-app directory: a `.pbf` is
hundreds of megabytes and the land dataset is over two gigabytes, and a developer
who already downloaded Switzerland from the CLI should not download it again
because they opened the app. The app reports all of them, with sizes and a Clear
button, in its "On this machine" card.

Built maps go somewhere a person can find, back up and copy to a card — that a
desktop app *has* a filesystem is most of the reason it exists (#894).

## Layout

| | |
|---|---|
| `main.rs` | the Tauri commands — one per FastAPI endpoint the dev server has, plus storage |
| `build_job.rs` | one build: download → `obc_pack::pipeline::pack` → events on a channel; the cancel token |
| `regions.rs` | the Geofabrik index, trimmed + simplified (through the GEOS the packer already links) |
| `content.rs` | presets, palette and the config schema — all baked in, none read off a repo |
| `storage.rs` | what is on this disk and how to get it back |
| `http.rs` | the list of hosts the app reaches; the bytes move through `obc_pack::net` |
| `paths.rs` | where things go, and why |

The window is granted `core:default` and nothing else: no filesystem plugin, no
shell, no HTTP. Every one of those policies is Rust code — `storage_clear` takes an
id from a fixed table rather than a path, and `reveal_file` refuses anything outside
the maps folder. Three hosts are reachable and no more: Geofabrik, the map catalog,
and `osmdata.openstreetmap.de` for the land dataset — that last one through the
packer, on the first build that needs land.

## Not here yet

- **Installers, signing, auto-update** — D3 (#908). `icons/` holds a placeholder
  mark generated by `icons/make_icons.py`; `tauri::generate_context!` will not
  compile without one, and shipping the default Tauri logo would be worse. D3 also
  decides **macOS universal vs arm64-only**: CI proves the Apple-silicon build by
  running it and the Intel slice by cross-*building* it, so both remain available,
  but nothing here `lipo`s them into one bundle. Linux distribution has the same
  shape of open question — the binary is GEOS-free but still dynamically links
  WebKitGTK, so a distribution-independent artifact means an AppImage or a Flatpak
  runtime, not just this file.
- **USB** — D4 (#909) swaps `nusb` in behind the platform seam's `device()`.
- **The ride library** — E2 (#912).
