# obc-desktop — the OpenBikeComputer desktop app

The web builder, shipped as an app: the same Svelte frontend in a Tauri v2 window,
with **`obc-pack` linked in as a library** instead of a Python server in front of a
subprocess (issue #906, phase D of #894).

What the concepts are and why the tiers split the way they do lives in the docs
site ([packer & routing](../../docs/content/software/packer-routing.md)). This file
is how you build and run it.

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
cargo run --no-default-features        # follows http://localhost:5173
```

## Checks

Standalone cargo root, like `obc-fw-nrf54l` and `obc-boot` — **not** a member of
the `firmware/` workspace, so it needs its own invocations (and gets its own CI
job):

```sh
cargo fmt                 # the third + fourth step of CLAUDE.md's fmt dance
cargo clippy --all-targets -- -D warnings
cargo test
```

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
| `main.rs` | the Tauri commands — one per FastAPI endpoint the dev server has, plus storage and USB |
| `build_job.rs` | one build: download → `obc_pack::pipeline::pack` → events on a channel; the cancel token |
| `regions.rs` | the Geofabrik index, trimmed + simplified (through the GEOS the packer already links) |
| `content.rs` | presets, palette and the config schema — all baked in, none read off a repo |
| `storage.rs` | what is on this disk and how to get it back |
| `http.rs` | every network call the app makes, in one auditable place |
| `paths.rs` | where things go, and why |
| `usb/` | native USB (`nusb`): device discovery, hot-plug, and two byte pipes — see below |

The window is granted `core:default` and nothing else: no filesystem plugin, no
shell, no HTTP. Every one of those policies is Rust code — `storage_clear` takes an
id from a fixed table rather than a path, `reveal_file` refuses anything outside the
maps folder, `usb_send_file` streams only from the app's own folders, and the only
hosts the app reaches are Geofabrik and the map catalog.

## USB

The system webview has no WebUSB (WKWebView, WebView2 and WebKitGTK all lack it),
so this app drives USB itself through `nusb`. **The protocol is not reimplemented
here**: the object model, the descriptors and the CRC are C3's TypeScript client
(`packer/web_builder/frontend/src/lib/usb/`), the same one the hosted site runs,
and `src/usb/` supplies the byte pipes underneath it. The concepts are in the docs
site ([companion link](../../docs/content/software/companion-link.md)).

Two planes, split by size:

| | carries | route |
|---|---|---|
| control | one framed message per operation, ≤ ~131 B | IPC, raw binary |
| bulk, small | routes, rides, catalogs, firmware images | IPC, raw `ArrayBuffer` bodies |
| bulk, by path | maps — hundreds of megabytes | `usb_send_file`: disk → endpoint, never through the webview |

### Platform permissions

macOS and Windows need nothing. Linux needs a udev rule, because usbfs nodes are
root-owned by default and the failure otherwise reads as a bug in the app:

```sh
sudo cp linux/99-openbikecomputer.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
# then unplug and re-plug the device
```

The file itself explains the choice of `TAG+="uaccess"` and why the other two
platforms are free.

### Checking it against a real device

There is no device in CI, so `cargo test` covers the endpoint-layout rule, the
CRC, the chunk arithmetic and the send-path policy; everything above the Tauri
command boundary is covered by `npm test` in the frontend, against C3's simulated
device. What only hardware answers — enumeration, hot-plug latency, short-packet
termination, throughput — is the on-glass recipe in the D4 PR (#909).

A quick smoke test with the device plugged in:

```sh
# The app should light up on its own within about a second of plugging in.
cargo run --release
```

If it does not, `RUST_LOG=nusb=debug cargo run --release` prints what the OS said.

## Not here yet

- **Cross-platform builds and GEOS vendoring** — D2 (#907). This crate links the
  system libGEOS through `obc-pack`, which is fine on a dev machine and is exactly
  what D2 exists to fix. `obc-pack`'s land-dataset bootstrap also still shells out
  to `curl` and `unzip`, which are not a given on Windows.
- **Installers, signing, auto-update** — D3 (#908). `icons/` holds a placeholder
  mark generated by `icons/make_icons.py`; `tauri::generate_context!` will not
  compile without one, and shipping the default Tauri logo would be worse.
- **The ride library** — E2 (#912). The download direction works today (rides are
  tens of kilobytes and ride the ordinary pipe); what E2 owns is the managed
  folder, the dedupe by `(serial, epoch, id)` and the ack-after-fsync.
- **A native file picker.** `usb_send_file` only streams from the app's own
  folders, so the file-path plane's first caller is E3's build-to-device (#913).
  A `.obcm` the rider picks in the window arrives as a `File` with no path and
  goes over the ordinary chunked pipe — correct, and flat in memory, just not the
  zero-copy path.
