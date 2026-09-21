# obc-desktop

The website's cell-catalog map builder, shipped in a Tauri v2 window. The Svelte coverage UI,
digest verification, corridor selection and wasm assembly engine are the same in both products.
Rust supplies native HTTPS, durable map-set output, USB and the ride library; it does not link
`obc-pack` and it does not process `.osm.pbf` files.

The architecture and the map pipeline are in the public docs. This file is the desktop build, run
and platform reference.

## Requirements

| | |
|---|---|
| Rust | stable, via `rustup` |
| Node | 22+, for the embedded frontend |
| Linux only | WebKitGTK for `wry`: `libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev libgtk-3-dev librsvg2-dev` |

No GEOS, CMake, libusb or Python dependency. `nusb` talks to each platform's native USB stack.

## Build and run

Build the two wasm bridges and the desktop frontend first, then run the standalone Rust crate:

```sh
cd builder/app
npm ci
npm run build:wasm
npm run build:desktop

cd ../../apps/obc-desktop
cargo run --release
```

`custom-protocol` is a default feature, so plain `cargo run` embeds `builder/app/dist/desktop`.
For the Vite live-reload loop:

```sh
# terminal 1, from apps/obc-desktop
npm --prefix ../../builder/app run dev -- --mode desktop

# terminal 2
cargo run --no-default-features
```

Packaged builds compile their catalog root from `OBC_CATALOG_URL`, which defaults to
`https://maps.openbikecomputer.com/cell-catalog/catalog.json`. A runtime variable of the same name
overrides it for testing an unpublished catalog:

```sh
OBC_CATALOG_URL=https://maps.example.test/catalog.json cargo run --release
```

The catalog root, every satellite and every cell are fetched through Rust. Object URLs must use
the configured root's HTTPS origin, with loopback HTTP allowed for local testing, so the webview
is never handed a general network proxy.

## Checks

This is a standalone cargo root, like `obc-fw-nrf54l` and `obc-boot`, so it needs its own checks:

```sh
cargo fmt -- --check
cargo check --release --locked
cargo clippy --release --locked --all-targets -- -D warnings
cargo test --release --locked -- --nocapture
```

The shared Svelte checks run from `builder/app`: `npm run check`, `npm test`, `npm run build:all`.

## Linux release-launch test

The `desktop-launch` CI job opens the real window under Xvfb, checks its `tauri://localhost`
origin, searches for Switzerland and adds it to the map, against a loopback catalog. To run it on
a Linux host with no OBC device attached, build the release app first, then from the repository
root:

```sh
sudo apt-get install webkit2gtk-driver xvfb imagemagick dbus-daemon
cargo install tauri-driver --version 2.0.6 --locked
python3 -m venv .venv
. .venv/bin/activate
pip install -r apps/obc-desktop/e2e/requirements.txt
xvfb-run -a dbus-run-session -- python3 apps/obc-desktop/e2e/launch.py
```

**Use a WebKit driver with the same version as the installed WebKitGTK runtime.** CI installs an
exactly matched pair and records both package versions. `dbus-run-session` gives the app and the
desktop portal services a private session bus with the Xvfb display.

`OBC_DESKTOP_BINARY` names an existing release executable. Evidence goes to
`target/desktop-launch`, or `OBC_DESKTOP_EVIDENCE`: `result.json`, native catalog request logs,
application and driver logs, rendered HTML and a screenshot, plus `failure.png` when a webview
session is available.

This suite covers Linux software launch and catalog integration only. It establishes no USB
permission, no enumeration with a physical device and no route upload. Windows launch is not
automated, and Tauri's native WebDriver route does not support macOS.

## Files and storage

| | |
|---|---|
| Assembled maps | `~/Documents/OpenBikeComputer/<map>/` |
| Pulled rides (GPX) | `~/Documents/OpenBikeComputer/rides/` — relocatable |
| Ride archive | `<app data>/ride-archive/` — internal, not relocatable |

Each completed assembly is a uniquely named folder. Files are written to `.part`, flushed and
atomically renamed before the assembly is reported complete. The website gets the same
worker-produced bytes through browser downloads.

The visible ride folder holds GPX files only; device ride objects and the index live in the
internal archive, and `src/rides.rs` documents the durable import and relocation rules. Catalog
cells are held only for the assembly run: there is no `.pbf`, land-polygon, Geofabrik-index or
persistent cell cache.

## Layout

| | |
|---|---|
| `src/main.rs` | Tauri commands and the deliberately small webview capability surface |
| `src/catalog.rs`, `src/http.rs` | Configured catalog root and same-origin native object reads |
| `src/map_output.rs` | Opaque output sessions and atomic map-set writes |
| `src/storage.rs`, `src/paths.rs` | Visible app-owned locations |
| `src/rides.rs` | Managed GPX library plus the durable ride archive |
| `src/usb/` | Native USB discovery and byte pipes beneath the shared TypeScript protocol |

The window is granted `core:default` and no filesystem, shell or HTTP plugin; those policies live
in Rust commands. `dragDropEnabled: false` in `tauri.conf.json` turns off Tauri's OS-level
interception so the shared HTML5 GPX drop targets get files normally.

## USB permissions

macOS and Windows need no extra setup. On Linux:

```sh
sudo cp linux/99-openbikecomputer.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
# unplug and reconnect the device
```

The device protocol stays the shared TypeScript implementation in `builder/app/src/lib/usb/`;
Rust supplies the native pipes only. Large app-owned files stream straight from the maps folder
without entering the webview. A newly assembled volume set streams to the device with the manifest
last, and the device page also accepts a standalone `.obcm` obtained elsewhere.

## Not here yet

Installers, signing and auto-update. A native arbitrary-file picker is not exposed; files selected
in the webview use the ordinary chunked transfer path.
