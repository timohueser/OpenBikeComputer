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

Nothing on that list is for USB: `nusb` is pure Rust and talks to each platform's
own stack, so there is no libusb to install or vendor. The one thing a *device*
needs is a Linux udev rule, which is a permission rather than a dependency — see
[Platform permissions](#platform-permissions).

## Build and run

The Rust side embeds the built frontend, so the frontend is built first:

```sh
cd builder/app
npm ci
npm run build:wasm        # the GPX↔OBCR + preset-preview bridges the app imports
npm run build:desktop     # → dist/desktop, which tauri.conf.json embeds

cd ../../../apps/obc-desktop
cargo run --release       # opens the window
```

`cargo run` is the whole story — there is no `tauri` CLI in this repo. That works
because `custom-protocol` is a **default feature** here, which is what tells Tauri
to embed `dist/desktop` rather than point the window at a dev server. The upstream
templates leave that feature off and have their CLI add it, which produces a blank
white window for anyone running plain `cargo`; this crate does not.

For a live-reload loop, opt out of it and run Vite instead:

```sh
npm --prefix ../../builder/app run dev -- --mode desktop
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
C++ library reached over a C ABI, so it packs `builder/tests/corpus/data/
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

Three tests are `#[ignore]`d because they pack a real region and therefore need a
warm cache — a `.pbf` under `~/.cache/obcm/pbf`, the Geofabrik index, and the
land-polygon dataset. They are the ones that answer #906's and E3 #913's
acceptance criteria:

```sh
# the app's map and the CLI's map, byte for byte, with digests — twice: once for a
# shipped preset (#906) and once for an edited style cropped to a box (E3 #913)
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
| Exported styles | `~/Documents/OpenBikeComputer/styles/` |
| Pulled rides (GPX) | `~/Documents/OpenBikeComputer/rides/` — **relocatable**, see below |
| Ride archive | `<app data>/ride-archive/` — internal, **not** relocatable |
| `.pbf` extracts | `~/.cache/obcm/pbf/` |
| Land-polygon dataset | `~/.cache/obcm/land/` |
| Geofabrik index | `~/.cache/obcm/geofabrik/` |

The caches are the **shared** ones (`OBCM_CACHE_DIR` overrides the root, exactly as
`builder/server/paths.py` reads it), not a per-app directory: a `.pbf` is
hundreds of megabytes and the land dataset is over two gigabytes, and a developer
who already downloaded Switzerland from the CLI should not download it again
because they opened the app. The app reports all of them, with sizes and a Clear
button, in its "On this machine" card.

Built maps go somewhere a person can find, back up and copy to a card — that a
desktop app *has* a filesystem is most of the reason it exists (#894).

The **ride library** (E2, #912) is split in two. The visible folder holds **only
GPX** — one `.gpx` per ride, the thing other software reads — and it is the one
folder the rider can move: "Change…" in the ride page opens the OS directory
chooser, moves the GPX files, and remembers the choice in `ride-library.json`
under the app's config directory (not in the library — a folder that named itself
could not be found once it moved). The device's own ride objects (`.obcride`) and
the `index.json` live in the internal **ride archive** under the app's data
directory; that store never moves with the folder, and a library written by an
older build (everything in the one visible folder) is migrated over — durably and
idempotently — the first time the library is opened. `rides.rs`'s module docs own
the layout, the migration and the reasons.

## Layout

| | |
|---|---|
| `main.rs` | the Tauri commands — one per FastAPI endpoint the dev server has, plus storage and USB |
| `build_job.rs` | one build: download → `obc_pack::pipeline::pack` → events on a channel; the cancel token |
| `regions.rs` | the Geofabrik index, trimmed + simplified (through the GEOS the packer already links) |
| `content.rs` | presets, palette and the config schema — all baked in, none read off a repo |
| `storage.rs` | what is on this disk and how to get it back |
| `http.rs` | the list of hosts the app reaches; the bytes move through `obc_pack::net` |
| `paths.rs` | where things go, and why |
| `rides.rs` | the ride library: the index, the durable write the `ackRides` waits on, and relocation |
| `usb/` | native USB (`nusb`): device discovery, hot-plug, and two byte pipes — see below |

The window is granted `core:default` and nothing else: no filesystem plugin, no
shell, no HTTP. Every one of those policies is Rust code — `storage_clear` takes an
id from a fixed table rather than a path, `reveal_file` refuses anything outside the
maps folder and the ride library, and `usb_send_file` streams only from the app's
own folders. Two plugins are registered (`opener`, `dialog`) and **neither grants
the webview anything**: both are called from Rust, so the frontend still names a
file and never a place — when a place has to be named, the OS's own chooser does
it with the person driving.
Three hosts are reachable and no more: Geofabrik, the map catalog, and
`osmdata.openstreetmap.de` for the land dataset — that last one through the
packer, on the first build that needs land.

The window also sets **`dragDropEnabled: false`**, which reads backwards and is
not. The flag governs Tauri's *own* OS-level drag-and-drop handler, not the
frontend's: while it is on, wry claims the webview as an `NSDraggingDestination`
and Tauri's handler reports every event as handled, so the file never reaches the
page at all. `RouteDrop`'s "Drop a GPX file here" would be dead here while
working on the hosted site — a silent tier difference with only the "Choose a
file…" fallback standing between it and a bug report. Nothing in this app listens
for `tauri://drag-drop`, so switching it off gives up nothing and hands the page
back the ordinary HTML5 `dragover`/`drop` events it is already written against.
Tauri's own docs mention only Windows here; the interception is the same on
macOS, which is where this was measured.

## USB

The system webview has no WebUSB (WKWebView, WebView2 and WebKitGTK all lack it),
so this app drives USB itself through `nusb`. **The protocol is not reimplemented
here**: the object model, the descriptors and the CRC are C3's TypeScript client
(`builder/app/src/lib/usb/`), the same one the hosted site runs,
and `src/usb/` supplies the byte pipes underneath it. The concepts are in the docs
site ([companion link](../../docs/content/software/companion-link.md)).

Two planes, split by size:

| | carries | route |
|---|---|---|
| control | one framed message per operation, ≤ ~131 B | IPC, raw binary |
| bulk, small | routes, rides, catalogs, firmware images | IPC, raw `ArrayBuffer` bodies |
| bulk, by path | maps — hundreds of megabytes | `usb_send_file`: disk → endpoint, never through the webview |

The third row's caller is the app's own build (E3, #913): a finished `.obcm` is in
the maps folder, which is one of the two roots `usb::sendable_path` allows, so
"Send to device" in the window is a 12-byte descriptor plus a progress channel and
the file never enters the webview. What that does **not** mean is a build streamed
into the cable with no file at all: the transfer descriptor announces the whole
object's length and CRC-32 before the first byte moves (spec §4.2), and neither is
known until the packer has written its last chunk. The `.obcm` is the product, not
an intermediate.

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

- **Installers, signing, auto-update** — D3 (#908). `icons/` holds a placeholder
  mark generated by `icons/make_icons.py`; `tauri::generate_context!` will not
  compile without one, and shipping the default Tauri logo would be worse. D3 also
  decides **macOS universal vs arm64-only**: CI proves the Apple-silicon build by
  running it and the Intel slice by cross-*building* it, so both remain available,
  but nothing here `lipo`s them into one bundle. Linux distribution has the same
  shape of open question — the binary is GEOS-free but still dynamically links
  WebKitGTK, so a distribution-independent artifact means an AppImage or a Flatpak
  runtime, not just this file. (USB adds nothing to that list: `nusb` is pure Rust
  and the udev rule above is a text file, not a runtime dependency.)
- **A native *file* picker.** E2 (#912) added a native **directory** chooser, for
  relocating the ride library — but only that one. `usb_send_file` still streams
  from the app's own folders, and a `.obcm` the rider picks in the window arrives
  as a `File` with no path and goes over the ordinary chunked pipe — correct, and
  flat in memory, just not the zero-copy path. Widening `sendable_path` to
  arbitrary selections wants the file half of the same dialog.
- **A native save dialog.** `save_style` writes exports into
  `OpenBikeComputer/styles/`, and a pulled ride's GPX lands in the ride library,
  rather than either asking where it should go. Not for want of a dialog now: a
  folder chosen in Rust is a policy you can read, and it is the one that keeps the
  window's capability set at `core:default`. The one thing neither is is the
  browser's `<a download>` — that is silently inert here (wry installs a download
  delegate only when the embedder supplies a handler), which is why an export goes
  through a command at all.
