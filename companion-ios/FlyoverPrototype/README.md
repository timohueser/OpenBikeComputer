# OBC Flyover prototype

A separate iPhone app for the Kandel ride. Requires iOS 17+, Xcode, XcodeGen, Node 22+
and a network connection. It does not link or replace the companion app.
It uses `org.openbikecomputer.spike.flyover`, so installation updates the existing Flyover
spike and uses the same free-signing app slot.

## Build and install

Run from this directory:

```sh
npm ci
npm test
xcodegen generate
open OBCFlyover.xcodeproj
```

Select the OBCFlyover scheme, your signing team and your iPhone. Run the Release build.
The generated project and bundled Cesium files are ignored. Change `project.yml` to change
the project. `npm ci` copies the pinned Cesium runtime and its notices into `Web/cesium`.

For a simulator build without signing:

```sh
xcodebuild build -quiet -project OBCFlyover.xcodeproj -scheme OBCFlyover \
  -configuration Release -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath DerivedData CODE_SIGNING_ALLOWED=NO
```

For web inspection, run `python3 -m http.server 8768 --bind 127.0.0.1 --directory Web`.
Open `http://127.0.0.1:8768`. Browser results do not measure iPhone performance.

## Controls

| Control | Action |
| --- | --- |
| Follow | Resume the route camera. It looks ahead and limits its turn rate. |
| Free camera | Hold the current camera during playback. Drag, pinch or use two fingers to tilt. |
| Overview | Fit the whole ride and enter free mode. |
| Profile | Seek to a distance and pause. |
| Play / Pause | Play the ride in 60 seconds at 1×. Replay starts at the beginning. |
| 1× / 0.5× | Select normal or half playback speed. |
| FPS target | Switch between 60 and 30 frames per second. |

Touching the map enters free mode. Returning from the background leaves playback paused.
The elevation profile uses GPX heights. The route line uses sampled terrain heights.

## Device measurement

The footer reports rendered frames per second, the 95th percentile frame interval and the
pending tile count over each five-second playback window. Rendering uses at most 1.5 pixels
per CSS pixel. It stops drawing an unchanged scene when paused.

The app writes the latest report and up to 120 playback windows to `Documents/performance.json`. Reports include the
render size, target rate, tile errors and the native thermal state. Xcode's console also
receives each report. The renderer runs in WebKit's separate process; these figures do not
measure its memory use. Use Instruments for memory and energy measurements.

Launch with `-benchmark` to start a full flight eight seconds after the route is ready.
Compare the same phone, orientation and FPS target. Tile loading and thermal state affect
the result. The initial flight includes network loading.

## Data and limits

CesiumJS is Apache-2.0 licensed. Its runtime is bundled; no remote app code is loaded.
Terrain and imagery come directly from Esri's public Terrain3D and World Imagery services.
The map retains Cesium's data attribution control. No route or photo file is uploaded.
Tile requests disclose the viewed map areas to the provider.

Provider terms are separate from the renderer license:
[Terrain3D](https://www.arcgis.com/home/item.html?id=7029fb60158543ad845c7e1527af11e4),
[World Imagery](https://www.arcgis.com/home/item.html?id=10df2279f9684e4a9f6a7f08febac2a9).
Production credentials, caching rights and video rights are not established by this prototype.

The app serves bundled files on a loopback-only HTTP port for Cesium workers. Remote data
uses HTTPS. It has no application backend and no persistent map download feature.
It includes one continuous GPX track. Photos, video export, terrain exaggeration and ride
import are outside this prototype. Terrain clearance and route draping are approximate.

## Wireframe preview

Open `Mocks/ride-replay.html` in a browser. The Preview menu selects the entry, camera,
photo and network states. Play, scrub, drag the map, and use Overview or Reset camera.
The gray terrain, photos and values are illustrative. This is an interaction mock, not
an alternate renderer or the installed app.
