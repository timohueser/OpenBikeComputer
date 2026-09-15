# OBCDevice

The bike computer's own firmware on an iPhone. The shared application and the real renderer run
over one persistent card file in `Application Support`, with the phone's GPS, compass, barometer
and battery on the sensor ports. Four touch pads are the four buttons.

It is a development tool. It puts the user interface, the tracking, the route following and the
ride recording in front of real sensors. It says nothing about the speed of the device, the power
use, the display driver or the Bluetooth link.

The Rust side is [`apps/obc-ios-host`](../../apps/obc-ios-host). This target is the shell around
it and links no part of `OBCKit`.

## Prerequisites

- Xcode 26 with an iOS simulator runtime.
- XcodeGen: `brew install xcodegen`.
- Rust with the two iOS targets. `rust-toolchain.toml` lists them, so `rustup` installs them.

## Build

```sh
obc ios-host                    # packs target/OBCHost.xcframework
cd companion-ios && xcodegen generate
```

Then open `OBCCompanion.xcodeproj` and run the `OBCDevice` scheme.

**Run `obc ios-host` once, with both slices, before the first build.** Xcode reads the XCFramework
while it plans the build, before the target's own script phases run. With no bundle at
`target/OBCHost.xcframework` the build stops with "There is no XCFramework found at ...".

From then on the target's pre-build script keeps the slice for the current destination fresh, one
build behind: the build links the library that was on disk when it started, so a change in the Rust
host reaches the app on the next run. A slice is never dropped — a simulator build repacks the
simulator slice and keeps the device one — so you can switch the destination between a simulator
and a phone at any time.

The simulator slice is `aarch64-apple-ios-sim` only, so the target excludes `x86_64` for the
simulator SDK. An Intel Mac cannot run this app.

## On a phone

Signing comes from the gitignored `project.local.yml`, as it does for the companion. Put your team
id there, run `xcodegen generate` again, then pick your phone as the destination.

The card is a 32 GiB sparse file and is marked as excluded from backup. Settings › General ›
iPhone Storage reports the bytes in use, not 32 GB.

## How a map and routes get in

The app downloads nothing. A file arrives in one of three ways:

- **AirDrop** from the Mac. Choose OBC Device as the receiving app.
- **The Files app.** Share an `.obcm`, `.obcr` or `.gpx` and choose OBC Device.
- **Finder.** Connect the phone with a cable, open it in Finder, select Files, and drop the file
  on the OBC Device row.

Every file lands in the app's `Documents/`. An `.obcm` offers "Use as map": the host closes, the
map is copied onto the card, and the host opens again. An `.obcr` or `.gpx` goes straight into the
route list.

On a first launch with no map on the card and exactly one `.obcm` in `Documents/`, the app takes
that map without asking.

The gear button opens the developer sheet: the `Documents/` list with Use as map, Import route and
Delete; the exported rides with a share button; the panel scale; the current screen name; the last
error from the host; and "Reset card", which deletes the card file.

## Where rides come out

A finished ride is written as GPX into `Documents/rides/`. The developer sheet shares one file.
The Files app and Finder file sharing show them all.

## Peak View

Peak View needs a map that carries an OBCT surface region. A map without one reports the panorama
as unavailable.

## The simulator

Build, install and permit the app:

```sh
xcrun simctl boot "iPhone 17 Pro"
open -a Simulator
tools/build-ios-host.sh --sim-only
cd companion-ios && xcodegen generate
xcodebuild build -project OBCCompanion.xcodeproj -scheme OBCDevice \
  -configuration Debug -destination 'platform=iOS Simulator,name=iPhone 17 Pro' \
  -derivedDataPath DerivedData CODE_SIGNING_ALLOWED=NO
xcrun simctl install booted DerivedData/Build/Products/Debug-iphonesimulator/OBCDevice.app
xcrun simctl privacy booted grant location com.openbikecomputer.device
```

Put a map and a route in `Documents/` before the first launch. The app then takes the map by
itself, which makes the run scriptable:

```sh
container=$(xcrun simctl get_app_container booted com.openbikecomputer.device data)
mkdir -p "$container/Documents"
cp apps/obc-sim/assets/grimsel-demo.obcm "$container/Documents/"
cp fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr "$container/Documents/"
xcrun simctl launch booted com.openbikecomputer.device
```

Stream the demo track as the phone's position:

```sh
python3 - <<'PY' > /tmp/waypoints.txt
import pathlib, re
gpx = pathlib.Path("fixtures/sources/sim-grimsel/tracks/grimsel-climb.gpx").read_text()
for lat, lon in re.findall(r'<trkpt lat="([-0-9.]+)" lon="([-0-9.]+)"', gpx):
    print(f"{lat},{lon}")
PY
xcrun simctl location booted start --speed=6 --interval=1 - < /tmp/waypoints.txt
```

The simulator has no barometer and no battery level. The app asks for Motion & Fitness, gets no
altitude, and falls back to the altitude of the fix, which a simulated position reports as zero.
Climb therefore stays at zero. Take a screenshot with
`xcrun simctl io booted screenshot shot.png`.
