# obc-desktop — the OpenBikeComputer desktop app

The website's cell-catalog map builder, shipped in a Tauri v2 window. The Svelte
coverage UI, digest verification, corridor selection, and wasm assembly engine are
the same in both products. Rust supplies native HTTPS, durable map-set output, USB,
and the ride library; it does not link `obc-pack` or process `.osm.pbf` files.

The architecture and map pipeline live in the public docs. This file is only the
desktop build/run and platform-specific reference.

## Requirements

| | |
|---|---|
| Rust | stable, via `rustup` |
| Node | 22+, for the embedded frontend |
| Linux only | WebKitGTK for `wry`: `libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev libgtk-3-dev librsvg2-dev` |

The app has no GEOS, CMake, libusb, or Python dependency. `nusb` talks to each
platform's native USB stack. Linux additionally needs the udev rule below.

## Build and run

Build the two wasm bridges and the desktop frontend first, then run the standalone
Rust crate:

```sh
cd builder/app
npm ci
npm run build:wasm
npm run build:desktop

cd ../../../apps/obc-desktop
cargo run --release
```

`custom-protocol` is a default feature, so plain `cargo run` embeds
`builder/app/dist/desktop`. For the Vite live-reload loop:

```sh
# terminal 1, from apps/obc-desktop
npm --prefix ../../builder/app run dev -- --mode desktop

# terminal 2
cargo run --no-default-features
```

Packaged builds compile their catalog root from `OBC_CATALOG_URL`; it defaults to
`https://maps.openbikecomputer.com/cell-catalog/catalog.json`. A runtime environment variable
with the same name overrides it for testing an unpublished catalog:

```sh
OBC_CATALOG_URL=https://maps.example.test/catalog.json cargo run --release
```

The catalog root, every satellite, and every cell are fetched through Rust. Object
URLs must use the configured root's HTTPS origin (loopback HTTP is allowed for local
testing), so the webview is not handed a general network proxy.

## Checks

This is a standalone cargo root, like `obc-fw-nrf54l` and `obc-boot`, and therefore
needs its own checks:

```sh
cargo fmt -- --check
cargo check --release --locked
cargo clippy --release --locked --all-targets -- -D warnings
cargo test --release --locked -- --nocapture
```

The shared Svelte tests and builds run from `builder/app`:

```sh
npm run check
npm test
npm run build:all
```

## Files and storage

Each completed assembly is a uniquely named folder below
`~/Documents/OpenBikeComputer/` (or the platform's Documents directory). Files are
written to `.part`, flushed, and atomically renamed before the assembly is reported
complete. The website receives the same worker-produced bytes through browser
downloads.

| | |
|---|---|
| Assembled maps | `~/Documents/OpenBikeComputer/<map>/` |
| Pulled rides (GPX) | `~/Documents/OpenBikeComputer/rides/` — relocatable |
| Ride archive | `<app data>/ride-archive/` — internal, not relocatable |

Catalog cells are currently held only for the assembly run; the desktop app does
not maintain `.pbf`, land-polygon, Geofabrik-index, or persistent cell caches.

The visible ride folder contains GPX files only. Device ride objects and the index
live in the internal archive; `src/rides.rs` documents the durable import and
relocation rules.

## Layout

| | |
|---|---|
| `src/main.rs` | Tauri commands and the deliberately small webview capability surface |
| `src/catalog.rs`, `src/http.rs` | configured catalog root and same-origin native object reads |
| `src/map_output.rs` | opaque output sessions and atomic map-set writes |
| `src/storage.rs`, `src/paths.rs` | visible app-owned locations |
| `src/rides.rs` | managed GPX library plus durable ride archive |
| `src/usb/` | native USB discovery and byte pipes beneath the shared TypeScript protocol |

The window is granted `core:default` and no filesystem, shell, or HTTP plugin.
Filesystem and network policies live in Rust commands. `dragDropEnabled: false` in
`tauri.conf.json` disables Tauri's OS-level interception so the shared HTML5 GPX
drop targets receive files normally.

## USB permissions

macOS and Windows need no extra setup. On Linux:

```sh
sudo cp linux/99-openbikecomputer.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
# unplug and reconnect the device
```

The device protocol remains the shared TypeScript implementation in
`builder/app/src/lib/usb/`; Rust only supplies the native pipes. Large app-owned
files can stream directly from the maps folder without entering the webview.

Newly assembled volume sets stream directly to the device with the manifest last.
The device page also accepts a standalone `.obcm` obtained elsewhere.

## Not here yet

- Installers, signing, and auto-update remain #908.
- A native arbitrary-file picker is not exposed; files selected in the webview use
  the ordinary chunked transfer path.
