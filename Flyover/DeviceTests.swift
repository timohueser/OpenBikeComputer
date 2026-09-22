import SwiftUI
import MapKit

// Self-driving device tests (-devtest tilt). Evidence goes to Documents/.

let docsDir = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]

func dlog(_ s: String, file: String = "devlog.txt") {
    print(s)
    let url = docsDir.appendingPathComponent(file)
    let line = Data((s + "\n").utf8)
    if let h = try? FileHandle(forWritingTo: url) { h.seekToEndOfFile(); h.write(line); try? h.close() } else { try? line.write(to: url) }
}

@MainActor func grabWindow(_ name: String) {
    guard let win = UIApplication.shared.connectedScenes.compactMap({ ($0 as? UIWindowScene)?.keyWindow }).first else { return }
    let img = UIGraphicsImageRenderer(bounds: win.bounds).image { _ in _ = win.drawHierarchy(in: win.bounds, afterScreenUpdates: true) }
    try? img.pngData()?.write(to: docsDir.appendingPathComponent(name))
    dlog("saved \(name)")
}

struct TiltProbeView: UIViewRepresentable {
    func makeUIView(context: Context) -> MKMapView {
        let v = MKMapView()
        v.showsCompass = false
        Task { @MainActor in await runTilt(v) }
        return v
    }
    func updateUIView(_ v: MKMapView, context: Context) {}

    @MainActor func runTilt(_ v: MKMapView) async {
        func sleep(_ s: Double) async { try? await Task.sleep(for: .milliseconds(Int(s * 1000))) }
        await sleep(2)
        // Mid Kandel climb, looking up toward the summit (SE).
        let climb = CLLocationCoordinate2D(latitude: 48.0700, longitude: 8.0050)
        let summit = CLLocationCoordinate2D(latitude: 48.0614, longitude: 8.0161)
        let cfgs: [(String, MKMapConfiguration)] = [
            ("imagery", MKImageryMapConfiguration(elevationStyle: .realistic)),
            ("hybrid", MKHybridMapConfiguration(elevationStyle: .realistic)),
            ("standard", MKStandardMapConfiguration(elevationStyle: .realistic)),
        ]
        dlog("tilt,config,distance,requested,applied@0.3s,applied@2s,centerDist,device=\(UIDevice.current.model) \(UIDevice.current.systemVersion)", file: "tilt.csv")
        var best = 0.0
        for (name, cfg) in cfgs {
            v.preferredConfiguration = cfg
            await sleep(2)
            for d in [500.0, 1500, 3000, 8000] {
                for p in [45.0, 60, 70, 80] {
                    v.camera = MKMapCamera(lookingAtCenter: climb, fromDistance: d, pitch: p, heading: 135)
                    await sleep(0.3)
                    let a = v.camera.pitch
                    await sleep(1.7)
                    let b = v.camera.pitch
                    if name == "imagery" { best = max(best, b) }
                    dlog(String(format: "tilt,%@,%.0f,%.0f,%.1f,%.1f,%.0f", name, d, p, a, b, v.camera.centerCoordinateDistance), file: "tilt.csv")
                }
            }
        }
        // Steepest imagery views toward the summit, after tiles settle.
        v.preferredConfiguration = MKImageryMapConfiguration(elevationStyle: .realistic)
        for d in [1500.0, 3000, 8000] {
            v.camera = MKMapCamera(lookingAtCenter: summit, fromDistance: d, pitch: 85, heading: 120)
            await sleep(10)
            dlog(String(format: "summit view d=%.0f applied pitch=%.1f", d, v.camera.pitch))
            grabWindow("device-summit-imagery-d\(Int(d)).png")
        }
        dlog("TILT DONE best imagery pitch=\(best)")
    }
}
