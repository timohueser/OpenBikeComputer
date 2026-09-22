import SwiftUI
import MapKit
import QuartzCore

// Launch args (UserDefaults): -mode frame|chunk|keyframe  -trail frame|10hz|chunked  -style imagery|hybrid|standard
//                             -autoplay YES  -seek <seconds>  -dist <m>  -pitch <deg>

enum CamMode: String, CaseIterable { case frame, chunk, keyframe }
enum TrailMode: String, CaseIterable { case frame, hz10 = "10hz", chunked, off }

@Observable @MainActor
final class Flyover: NSObject {
    let track = Track(raw: GPXParser.parse(Bundle.main.url(forResource: "kandel", withExtension: "gpx")!))
    var mode = CamMode(rawValue: UserDefaults.standard.string(forKey: "mode") ?? "") ?? .frame
    var trailMode = TrailMode(rawValue: UserDefaults.standard.string(forKey: "trail") ?? "") ?? .frame
    let dist = UserDefaults.standard.double(forKey: "dist") > 0 ? UserDefaults.standard.double(forKey: "dist") : 1800
    let pitch = UserDefaults.standard.double(forKey: "pitch") > 0 ? UserDefaults.standard.double(forKey: "pitch") : 70

    var t = 0.0
    var playing = false
    var position: MapCameraPosition = .automatic
    var trail: [CLLocationCoordinate2D] = []          // frame / 10hz modes
    var chunkCount = 0                                  // chunked mode: committed chunks
    var liveTail: [CLLocationCoordinate2D] = []         // chunked mode: last chunk boundary -> head
    var head = CLLocationCoordinate2D()
    var activePhoto: Int?
    var kfTrigger = 0
    var kfStart = 0.0

    static let chunk = 100 // samples (1 km)
    static let shared = Flyover()

    @ObservationIgnored private var link: CADisplayLink?
    @ObservationIgnored private var last: CFTimeInterval = 0
    @ObservationIgnored private var lastTrailBucket = -1
    @ObservationIgnored private var lastChunkPush = -1.0
    // Stats: main-thread tick intervals.
    @ObservationIgnored private var ticks = 0
    @ObservationIgnored private var hitches = 0
    @ObservationIgnored private var maxDt = 0.0
    @ObservationIgnored private var updCost = 0.0
    @ObservationIgnored private var statStart: CFTimeInterval = 0

    override init() {
        super.init()
        UIApplication.shared.isIdleTimerDisabled = true // unattended device runs: auto-lock would suspend the app
        t = UserDefaults.standard.double(forKey: "seek")
        update(force: true)
        let l = CADisplayLink(target: self, selector: #selector(tick(_:)))
        l.preferredFrameRateRange = .init(minimum: 60, maximum: 120, preferred: 120)
        l.add(to: .main, forMode: .common)
        link = l
        let ud = UserDefaults.standard
        if ud.double(forKey: "lat") != 0 { // probe mode: fixed camera anywhere
            position = .camera(MapCamera(centerCoordinate: .init(latitude: ud.double(forKey: "lat"), longitude: ud.double(forKey: "lon")), distance: dist, heading: ud.double(forKey: "heading"), pitch: pitch))
        }
        var mh = 0.0
        for k in 1..<Int(track.duration * 60) {
            let a = track.heading(atTime: Double(k - 1) / 60), b = track.heading(atTime: Double(k) / 60)
            mh = max(mh, abs(b - a) * 60)
        }
        print("planned max heading rate: \(Int(mh)) deg/s")
        print("track: \(track.coords.count) samples, \(Int(track.length)) m, duration \(track.duration) s, photos at \(track.photos.map { $0.holdStart })")
        switch ud.string(forKey: "export") {
        case "replaykit": DispatchQueue.main.asyncAfter(deadline: .now() + 4) { ExportProbe.replayKit(self) }
        case "capture": DispatchQueue.main.asyncAfter(deadline: .now() + 4) { ExportProbe.replayCapture(self) }
        case "calib": Task { await ExportProbe.calibDump(self) }
        case "projcheck": Task { await ExportProbe.projCheck(self) }
        case "snapshot": Task { await ExportProbe.snapshots(self) }
        default: break
        }
        if UserDefaults.standard.bool(forKey: "autoplay") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 4) { self.play() }
        }
    }

    private func writePacing() {
        guard !frameDts.isEmpty else { return }
        let s = frameDts.sorted()
        let dropped = frameDts.reduce(0) { $0 + max(0, Int(($1 * 60).rounded()) - 1) }
        let long = frameDts.filter { $0 > 1.5 / 60 }.count
        dlog(String(format: "PACING engine=%@ pitchReq=%.0f pitchApplied(min/max)=%.1f/%.1f dist=%.0f frames=%d wall=%.1fs dropped=%d (%.1f%%) longFrames=%d p50=%.1fms p99=%.1fms max=%.1fms",
                    UserDefaults.standard.string(forKey: "engine") ?? "swiftui", pitch, pitchSamples.min() ?? -1, pitchSamples.max() ?? -1, dist, frameDts.count,
                    frameDts.reduce(0, +), dropped, Double(dropped) / Double(frameDts.count + dropped) * 100, long,
                    s[s.count / 2] * 1000, s[s.count * 99 / 100] * 1000, s.last! * 1000))
        frameDts.removeAll()
        if UserDefaults.standard.string(forKey: "devtest") == "pace" {
            Task { @MainActor in
                for (k, st) in [6.0, 20.0, 26.0, 40.0].enumerated() {
                    self.seek(st)
                    try? await Task.sleep(for: .seconds(6))
                    grabWindow("device-pace-\(k)-t\(Int(st)).png")
                }
                dlog("PACE DONE")
            }
        }
    }

    func camera(at t: Double) -> MapCamera {
        let fi = track.index(at: t)
        let h = track.heading(atTime: t)
        // Center a bit ahead of the rider so the rider sits in the lower part of the screen.
        let c = offset(track.coord(at: fi), meters: dist * 0.18, heading: h)
        return MapCamera(centerCoordinate: c, distance: dist, heading: h.truncatingRemainder(dividingBy: 360), pitch: pitch)
    }

    func play() {
        if t >= track.duration { t = 0 }
        playing = true
        lastChunkPush = -1
        if mode == .keyframe { kfStart = t; kfTrigger += 1 }
    }

    func pause() {
        playing = false
        position = .camera(camera(at: t)) // also used to test whether it interrupts the keyframe animator
    }

    func seek(_ nt: Double) {
        playing = false
        t = nt
        update(force: true)
        position = .camera(camera(at: t))
        onFrame?()
    }

    @objc private func tick(_ l: CADisplayLink) {
        let now = l.timestamp
        let dt = last == 0 ? 0 : now - last
        last = now
        if playing {
            ticks += 1
            if dt > 1.5 * (l.targetTimestamp - l.timestamp) && dt > 0.02 { hitches += 1 }
            maxDt = max(maxDt, dt)
            if statStart == 0 { statStart = now }
            t = min(track.duration, t + dt)
            if dt > 0 { frameDts.append(dt) }
            if let mv = mapView, frameDts.count % 60 == 0 { pitchSamples.append(mv.camera.pitch) }
            if t >= track.duration { writePacing() }
            sampleShown(now)
            onFrame?()
            let pa = UserDefaults.standard.double(forKey: "pauseAt")
            if pa > 0, t >= pa, !pausedOnce {
                pausedOnce = true
                pause()
                let at = shown?.centerCoordinate
                DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
                    print(String(format: "[pause test mode=%@] shown camera moved %.0f m in the 2 s after pause()", self.mode.rawValue, haversine(at!, self.shown!.centerCoordinate)))
                }
            }
            let s = CACurrentMediaTime()
            update(force: false)
            updCost += CACurrentMediaTime() - s
            if t >= track.duration { playing = false }
            if now - statStart > 5 || !playing {
                print(String(format: "[stats mode=%@ trail=%@] t=%.1f ticks=%d fps=%.1f hitches=%d maxDt=%.0fms avgUpdate=%.2fms trailPts=%d",
                             mode.rawValue, trailMode.rawValue, t, ticks, Double(ticks) / max(0.001, now - statStart), hitches, maxDt * 1000,
                             updCost / Double(max(1, ticks)) * 1000, trail.count + chunkCount * Flyover.chunk + liveTail.count))
                ticks = 0; hitches = 0; maxDt = 0; updCost = 0; statStart = now
            }
        }
    }

    // Smoothness probe: per-callback ground speed of the camera the map actually shows, versus its 15-sample moving average.
    @ObservationIgnored private var cam: [(t: CFTimeInterval, c: CLLocationCoordinate2D, h: Double)] = []
    @ObservationIgnored private var shown: MapCamera?
    @ObservationIgnored private var drift: [Double] = []
    @ObservationIgnored private var pausedOnce = false
    @ObservationIgnored var onFrame: (() -> Void)?
    @ObservationIgnored weak var mapView: MKMapView?
    @ObservationIgnored private var frameDts: [Double] = []
    @ObservationIgnored private var pitchSamples: [Double] = []
    func camLog(_ c: MapCamera) { shown = c }
    private func sampleShown(_ now: CFTimeInterval) {
        guard let c = shown, playing else { return }
        cam.append((now, c.centerCoordinate, c.heading))
        let e = haversine(c.centerCoordinate, camera(at: t).centerCoordinate)
        drift.append(e)
        guard cam.count > 1, cam.last!.t - cam.first!.t > 5 else { return }
        var v: [Double] = [], hr: [Double] = []
        for k in 1..<cam.count {
            let dt = cam[k].t - cam[k - 1].t
            guard dt > 0 else { continue }
            v.append(haversine(cam[k - 1].c, cam[k].c) / dt)
            var dh = cam[k].h - cam[k - 1].h; if dh > 180 { dh -= 360 }; if dh < -180 { dh += 360 }
            hr.append(abs(dh) / dt)
        }
        var res = 0.0, mean = 0.0, zeros = 0
        for k in 0..<v.count {
            let lo = max(0, k - 7), hi = min(v.count - 1, k + 7)
            let avg = v[lo...hi].reduce(0, +) / Double(hi - lo + 1)
            res += (v[k] - avg) * (v[k] - avg); mean += v[k]
            if v[k] < 0.2 * avg { zeros += 1 }
        }
        mean /= Double(v.count)
        print(String(format: "[smooth mode=%@] callbacks=%d (%.0f/s) speedJitter=%.1f%% stalls=%d maxHeadingRate=%.0f deg/s",
                     mode.rawValue, cam.count, Double(cam.count) / (cam.last!.t - cam.first!.t), sqrt(res / Double(v.count)) / mean * 100, zeros, hr.max() ?? 0))
        print(String(format: "    offset shown-vs-playhead camera: mean=%.0f m max=%.0f m (ground speed ~%.0f m/s)", drift.reduce(0, +) / Double(drift.count), drift.max() ?? 0, mean))
        cam.removeAll(); drift.removeAll()
    }

    private func update(force: Bool) {
        let fi = track.index(at: t)
        head = track.coord(at: fi)
        let p = track.photos.firstIndex { t >= $0.holdStart && t < $0.holdEnd }
        if p != activePhoto { activePhoto = p }

        // Trail.
        let i = Int(fi)
        switch trailMode {
        case .frame:
            trail = Array(track.coords[0...i]) + [head]
        case .hz10:
            let b = Int(t * 10)
            if b != lastTrailBucket || force { lastTrailBucket = b; trail = Array(track.coords[0...i]) + [head] }
        case .chunked:
            let c = i / Flyover.chunk
            if c != chunkCount { chunkCount = c }
            liveTail = Array(track.coords[(c * Flyover.chunk)...i]) + [head]
        case .off: break
        }

        // Camera.
        guard playing || force else { return }
        switch mode {
        case .frame:
            position = .camera(camera(at: t))
        case .chunk:
            // Hand MapKit a 0.5 s linear animation to the camera 0.5 s ahead; MapKit interpolates in between.
            if lastChunkPush < 0 || t - lastChunkPush >= 0.5 {
                lastChunkPush = t
                withAnimation(.linear(duration: 0.5)) { position = .camera(camera(at: min(track.duration, t + 0.5))) }
            }
        case .keyframe:
            if force { position = .camera(camera(at: t)) }
        }
    }
}

struct ContentView: View {
    @State var m = Flyover.shared // @State(initialValue:) re-runs init on every struct init; a class with a display link must be created once
    @State var style = UserDefaults.standard.string(forKey: "style") ?? "imagery"
    @State var engine = UserDefaults.standard.string(forKey: "engine") ?? "swiftui"
    @State var chrome = !UserDefaults.standard.bool(forKey: "nochrome")

    var mapStyle: MapStyle {
        switch style {
        case "hybrid": .hybrid(elevation: .realistic)
        case "standard": .standard(elevation: .realistic)
        default: .imagery(elevation: .realistic)
        }
    }

    var body: some View {
        Group {
            if engine == "uikit" {
                UIKitFlyover(m: m).ignoresSafeArea()
                    .overlay {
                        if let k = m.activePhoto { photoCard(k).transition(.scale.combined(with: .opacity)) }
                    }
                    .animation(.spring, value: m.activePhoto)
            } else {
                swiftUIMap
            }
        }
        .overlay(alignment: .top) {
            if chrome {
                VStack(spacing: 6) {
                    Picker("Engine", selection: $engine) { ForEach(["swiftui", "uikit"], id: \.self) { Text($0) } }
                    Picker("Camera", selection: $m.mode) { ForEach(CamMode.allCases, id: \.self) { Text($0.rawValue) } }
                    Picker("Trail", selection: $m.trailMode) { ForEach(TrailMode.allCases, id: \.self) { Text($0.rawValue) } }
                    Picker("Style", selection: $style) { ForEach(["imagery", "hybrid", "standard"], id: \.self) { Text($0) } }
                }
                .pickerStyle(.segmented).padding(8).background(.ultraThinMaterial, in: .rect(cornerRadius: 12)).padding(.horizontal)
            }
        }
        .overlay(alignment: .bottom) {
            if chrome {
                VStack {
                    Slider(value: Binding(get: { m.t }, set: { m.seek($0) }), in: 0...m.track.duration)
                    HStack(spacing: 24) {
                        Button { m.seek(0) } label: { Image(systemName: "backward.end.fill") }
                        Button { m.playing ? m.pause() : m.play() } label: { Image(systemName: m.playing ? "pause.fill" : "play.fill") }
                        Text(String(format: "%.1f / %.0f s", m.t, m.track.duration)).monospacedDigit().font(.caption)
                    }
                    .font(.title2)
                }
                .padding().background(.ultraThinMaterial, in: .rect(cornerRadius: 16)).padding()
            }
        }
    }

    func photoCard(_ k: Int) -> some View {
        VStack(spacing: 4) {
            Image(systemName: "photo.fill").font(.system(size: 44)).foregroundStyle(.white)
                .frame(width: 120, height: 80).background(.blue.gradient, in: .rect(cornerRadius: 10))
            Text("Photo \(k + 1)").font(.caption.bold())
        }
        .padding(6).background(.regularMaterial, in: .rect(cornerRadius: 14))
    }

    var swiftUIMap: some View {
        Map(position: $m.position) {
            MapPolyline(coordinates: m.track.coords).stroke(.white.opacity(0.45), lineWidth: 3)
            switch m.trailMode {
            case .frame, .hz10:
                MapPolyline(coordinates: m.trail).stroke(.orange, lineWidth: 5)
            case .chunked:
                ForEach(0..<m.chunkCount, id: \.self) { c in
                    MapPolyline(coordinates: Array(m.track.coords[(c * Flyover.chunk)...((c + 1) * Flyover.chunk)])).stroke(.orange, lineWidth: 5)
                }
                MapPolyline(coordinates: m.liveTail).stroke(.orange, lineWidth: 5)
            case .off: EmptyMapContent()
            }
            ForEach(Array(m.track.photos.enumerated()), id: \.offset) { k, p in
                Annotation("", coordinate: m.track.coords[p.index]) {
                    if m.activePhoto == k {
                        photoCard(k).transition(.scale.combined(with: .opacity))
                    } else {
                        Image(systemName: "camera.circle.fill").font(.title).foregroundStyle(.white, .blue)
                    }
                }
            }
            Annotation("", coordinate: m.head) {
                Circle().fill(.orange).stroke(.white, lineWidth: 3).frame(width: 18, height: 18)
            }
        }
        .mapStyle(mapStyle)
        .mapControlVisibility(.hidden)
        .onMapCameraChange(frequency: .continuous) { c in
            m.camLog(c.camera)
        }
        .onMapCameraChange(frequency: .onEnd) { c in
            print(String(format: "[cam] pitch=%.1f heading=%.1f dist=%.0f center=%.5f,%.5f", c.camera.pitch, c.camera.heading, c.camera.distance, c.camera.centerCoordinate.latitude, c.camera.centerCoordinate.longitude))
        }
        .mapCameraKeyframeAnimator(trigger: m.kfTrigger) { _ in
            keyframes(from: m.kfStart)
        }
        .animation(.spring, value: m.activePhoto)
        .ignoresSafeArea()
    }

    /// Keyframes for the rest of the ride: one knot every 0.5 s plus the photo hold boundaries.
    @KeyframesBuilder<MapCamera>
    func keyframes(from t0: Double) -> some Keyframes<MapCamera> {
        let times = keyTimes(from: t0)
        let cams = times.map { m.camera(at: $0) }
        let heads = times.map { m.track.heading(atTime: $0) } // unwrapped, avoids 350->10 spins
        KeyframeTrack(\MapCamera.centerCoordinate) {
            for k in 1..<times.count { LinearKeyframe(cams[k].centerCoordinate, duration: times[k] - times[k - 1]) }
        }
        KeyframeTrack(\MapCamera.heading) {
            for k in 1..<times.count { CubicKeyframe(heads[k], duration: times[k] - times[k - 1]) }
        }
        KeyframeTrack(\MapCamera.distance) { LinearKeyframe(m.dist, duration: 0.5) }
        KeyframeTrack(\MapCamera.pitch) { LinearKeyframe(m.pitch, duration: 0.5) }
    }

    func keyTimes(from t0: Double) -> [Double] {
        var ts = stride(from: t0, to: m.track.duration, by: 0.5).map { $0 }
        ts += m.track.photos.flatMap { [$0.holdStart, $0.holdEnd] }.filter { $0 > t0 }
        ts.append(m.track.duration)
        ts.sort()
        return ts.reduce(into: [Double]()) { if $0.isEmpty || $1 - $0.last! > 0.01 { $0.append($1) } }
    }
}

@main
struct FlyoverApp: App {
    var body: some Scene {
        WindowGroup {
            let ud = UserDefaults.standard
            if ud.string(forKey: "devtest") == "tilt" {
                TiltProbeView().ignoresSafeArea()
            } else if ud.bool(forKey: "uikitfly") {
                UIKitFlyover(m: Flyover.shared).ignoresSafeArea()
                    .overlay(alignment: .bottom) { Text(String(format: "UIKit %.1f s", Flyover.shared.t)).padding().background(.ultraThinMaterial) }
            } else if ud.bool(forKey: "uikit") {
                UIKitProbe(center: .init(latitude: ud.double(forKey: "lat"), longitude: ud.double(forKey: "lon")),
                           dist: ud.double(forKey: "dist"), pitch: ud.double(forKey: "pitch"), heading: ud.double(forKey: "heading")).ignoresSafeArea()
            } else { ContentView() }
        }
    }
}
