# Flyover spike (archive)

This branch is an archive. It never merges. It holds the throwaway iOS demo that proved the ride
flyover: a 3D satellite camera flight along a recorded ride, with a growing trail, photo pauses and
video export. The results and the decision are in issue #2081. The feature is in the backlog
(#2082, #2083).

## Build and run

Needs Xcode 26 and XcodeGen. The app plays `Flyover/Resources/kandel.gpx`.

```sh
xcodegen generate
xcodebuild -project Flyover.xcodeproj -scheme Flyover -destination 'platform=iOS Simulator,name=iPhone 17 Pro' build
```

For a device, set `DEVELOPMENT_TEAM` in `project.yml`, then build with
`-destination id=<udid> -allowProvisioningUpdates`. The Simulator caps the satellite tilt at
about 35°. Judge the 3D look on a device only.

Launch arguments (read through `UserDefaults`):

| Argument | Values |
| --- | --- |
| `-engine` | `swiftui`, `uikit` |
| `-mode` | `frame`, `chunk`, `keyframe` (camera strategy) |
| `-trail` | `frame`, `10hz`, `chunked`, `off` (SwiftUI trail updates) |
| `-style` | `imagery`, `hybrid`, `standard` (all with realistic elevation) |
| `-dist`, `-pitch` | camera distance in m, tilt in degrees |
| `-autoplay YES`, `-seek <s>`, `-nochrome YES` | playback control, hide the controls |
| `-devtest` | `tilt` (tilt table), `pace` (frame pacing) |
| `-export` | `replaykit`, `capture`, `calib`, `projcheck`, `snapshot` |

`run-combo.sh` records a Simulator run and prints the frame gaps. `calib_fit.py` fits the MapKit
camera from `evidence/calib/calib.json`.

## Files

| File | What it proves |
| --- | --- |
| `Flyover/UIKitProbe.swift` | The chosen player: `MKMapView`, camera set each frame from a `CADisplayLink`, `strokeEnd` trail |
| `Flyover/Track.swift` | GPX parse, pacing, heading smoothed over time (300 m look-ahead, 40°/s cap) |
| `Flyover/TerrainProjection.swift` | The fitted MapKit camera (30° vertical FOV) plus GPX elevations, to place the trail on a tilted view |
| `Flyover/Export.swift` | ReplayKit recording and the `MKMapSnapshotter` + `AVAssetWriter` renderer |
| `Flyover/DeviceTests.swift` | The self-driving device tests |
| `evidence/` | Compressed screenshots and videos. `device/` is the iPhone 13 run. |

## What will bite you

- The live satellite tilt depends on the camera distance: 70° at 500 m, 60° at 1500 m, 35° from
  3000 m. `MKMapSnapshotter` has the same limits.
- A SwiftUI `MapPolyline` trail lags the camera by hundreds of ms. Use UIKit.
- On a device, the `strokeEnd` trail still lags by 100–300 ms. Draw it in a `CAShapeLayer` with
  `TerrainProjection`.
- `stopRecording(withOutput:)` fails with -5835 when it writes into Documents. Write to the temp
  directory.
- `snapshot.point(for:)` ignores terrain height. The trail is about 100 px off at 60° without
  `TerrainProjection`.
- `@State var model = SomeClass()` runs the initializer on every view rebuild.
