import MapKit
import ReplayKit
import AVFoundation
import UIKit

// Video export probes. -export replaykit | snapshot
@MainActor
enum ExportProbe {
    static var docs: URL { FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0] }

    // 1. ReplayKit: record the app's own screen while the flyover plays. The first run shows a system prompt.
    static func replayKit(_ m: Flyover, seconds: Double = 15) {
        let r = RPScreenRecorder.shared()
        r.isMicrophoneEnabled = false
        dlog("[replaykit] isAvailable=\(r.isAvailable)")
        var started = false
        let asked = CACurrentMediaTime()
        dlog("[replaykit] WAITING FOR RECORD PERMISSION (tap 'Record Screen' on the phone, up to 90 s)")
        r.startRecording { err in
            DispatchQueue.main.async {
                dlog(String(format: "[replaykit] startRecording handler after %.1fs error=%@", CACurrentMediaTime() - asked, String(describing: err)))
                guard err == nil else { dlog("[replaykit] falling back to startCapture"); replayCapture(m); return }
                started = true
                m.play()
                DispatchQueue.main.asyncAfter(deadline: .now() + seconds) {
                    // Writing straight into Documents fails with RPRecordingErrorDomain -5835 (file permission); use tmp and move.
                    let tmp = FileManager.default.temporaryDirectory.appendingPathComponent("replaykit-\(Int(Date().timeIntervalSince1970)).mov")
                    r.stopRecording(withOutput: tmp) { err in
                        let url = docs.appendingPathComponent("replaykit.mov")
                        try? FileManager.default.removeItem(at: url)
                        try? FileManager.default.moveItem(at: tmp, to: url)
                        let size = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? Int) ?? -1
                        dlog("[replaykit] stopRecording error=\(String(describing: err)) bytes=\(size)")
                        if err != nil || size <= 0 {
                            dlog("[replaykit] falling back to startCapture + AVAssetWriter")
                            DispatchQueue.main.async { m.seek(14); replayCapture(m) }
                        } else { dlog("REPLAYKIT DONE") }
                    }
                }
            }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 90) {
            if !started { dlog("[replaykit] PERMISSION NEVER GRANTED within 90 s"); r.discardRecording {} }
        }
    }

    // 1b. ReplayKit startCapture: raw sample buffers into our own AVAssetWriter.
    static func replayCapture(_ m: Flyover) {
        let r = RPScreenRecorder.shared()
        let url = docs.appendingPathComponent("replaykit-capture.mp4")
        try? FileManager.default.removeItem(at: url)
        var writer: AVAssetWriter?, input: AVAssetWriterInput?
        var frames = 0, savedFirst = false
        r.startCapture(handler: { sb, type, err in
            guard type == .video, CMSampleBufferDataIsReady(sb) else { return }
            if writer == nil, let fd = CMSampleBufferGetFormatDescription(sb) {
                let dim = CMVideoFormatDescriptionGetDimensions(fd)
                dlog("[capture] first buffer \(dim.width)x\(dim.height)")
                let w = try! AVAssetWriter(outputURL: url, fileType: .mp4)
                let i = AVAssetWriterInput(mediaType: .video, outputSettings: [AVVideoCodecKey: AVVideoCodecType.h264, AVVideoWidthKey: dim.width, AVVideoHeightKey: dim.height])
                i.expectsMediaDataInRealTime = true
                w.add(i); w.startWriting(); w.startSession(atSourceTime: CMSampleBufferGetPresentationTimeStamp(sb))
                writer = w; input = i
            }
            if !savedFirst, frames == 60, let pb = CMSampleBufferGetImageBuffer(sb) {
                savedFirst = true
                let ci = CIImage(cvPixelBuffer: pb)
                if let cg = CIContext().createCGImage(ci, from: ci.extent) { try? UIImage(cgImage: cg).pngData()?.write(to: docs.appendingPathComponent("replaykit-capture-frame.png")) }
            }
            if input?.isReadyForMoreMediaData == true { input?.append(sb); frames += 1 }
        }, completionHandler: { err in
            dlog("[capture] startCapture error=\(String(describing: err))")
            DispatchQueue.main.async { m.play() }
        })
        // Alternative live grab: drawHierarchy of the window. Does the Metal map content come through?
        DispatchQueue.main.asyncAfter(deadline: .now() + 4) {
            let win = UIApplication.shared.connectedScenes.compactMap { ($0 as? UIWindowScene)?.keyWindow }.first!
            var ts: [Double] = []
            var img: UIImage?
            for after in [false, true] {
                let s = CACurrentMediaTime()
                img = UIGraphicsImageRenderer(bounds: win.bounds).image { _ in _ = win.drawHierarchy(in: win.bounds, afterScreenUpdates: after) }
                ts.append(CACurrentMediaTime() - s)
                try? img?.pngData()?.write(to: docs.appendingPathComponent("drawhierarchy-after\(after).png"))
            }
            dlog(String(format: "[drawHierarchy] afterScreenUpdates=false %.0fms, true %.0fms", ts[0] * 1000, ts[1] * 1000))
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 20) {
            r.stopCapture { _ in
                input?.markAsFinished()
                writer?.finishWriting {
                    let size = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? Int) ?? -1
                    dlog("[capture] frames=\(frames) bytes=\(size) status=\(writer?.status.rawValue ?? -1) err=\(String(describing: writer?.error))")
                    dlog("CAPTURE DONE")
                }
                if writer == nil { dlog("[capture] no video buffers arrived") }
            }
        }
    }

    // 2. Offline: one MKMapSnapshotter image per frame, trail projected with terrain height (TerrainCamera), into AVAssetWriter.
    static func snapshots(_ m: Flyover) async {
        let ud = UserDefaults.standard
        let t0 = ud.double(forKey: "exportFrom"), seconds = ud.double(forKey: "exportSeconds") > 0 ? ud.double(forKey: "exportSeconds") : m.track.duration - t0
        let fps = 30.0, dist = 1500.0, pitch = 60.0 // MapKit allows 60° only up to ~1.5 km; farther it clamps to 35°
        let size = CGSize(width: 393, height: 852), scale = 2.0
        let px = CGSize(width: size.width * scale, height: size.height * scale)
        let url = docs.appendingPathComponent("snapshot-export.mp4")
        try? FileManager.default.removeItem(at: url)
        let w = try! AVAssetWriter(outputURL: url, fileType: .mp4)
        let input = AVAssetWriterInput(mediaType: .video, outputSettings: [
            AVVideoCodecKey: AVVideoCodecType.h264, AVVideoWidthKey: px.width, AVVideoHeightKey: px.height])
        let ad = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: input, sourcePixelBufferAttributes: [
            kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
            kCVPixelBufferWidthKey as String: px.width, kCVPixelBufferHeightKey as String: px.height])
        w.add(input)
        w.startWriting()
        w.startSession(atSourceTime: .zero)
        let fmt = UIGraphicsImageRendererFormat(); fmt.scale = scale
        let renderer = UIGraphicsImageRenderer(size: size, format: fmt)
        let wall = CACurrentMediaTime()
        let n = Int(seconds * fps)
        var times: [Double] = [], failed = 0
        for f in 0..<n {
            let t = t0 + Double(f) / fps
            let fi = m.track.index(at: t), rider = m.track.coord(at: fi), h = m.track.heading(atTime: t)
            let o = MKMapSnapshotter.Options()
            o.camera = MKMapCamera(lookingAtCenter: rider, fromDistance: dist, pitch: pitch, heading: h)
            o.preferredConfiguration = MKImageryMapConfiguration(elevationStyle: .realistic)
            o.size = size
            o.traitCollection = UITraitCollection(displayScale: scale)
            let start = CACurrentMediaTime()
            guard let snap = try? await MKMapSnapshotter(options: o).start() else { failed += 1; continue }
            times.append(CACurrentMediaTime() - start)
            let i0 = Int(fi), e = m.track.ele[i0] + (m.track.ele[min(i0 + 1, m.track.ele.count - 1)] - m.track.ele[i0]) * (fi - Double(i0))
            let cam = TerrainCamera(center: rider, centerEle: e, distance: dist, pitch: pitch, heading: h, size: size)
            let photo = m.track.photos.firstIndex { t >= $0.holdStart && t < $0.holdEnd }
            let img = renderer.image { ctx in
                snap.image.draw(at: .zero) // includes the Apple Maps logo
                let g = ctx.cgContext
                g.setLineCap(.round); g.setLineJoin(.round)
                var started = false
                for i in 0...i0 {
                    guard let q = cam.project(m.track.coords[i], ele: m.track.ele[i]), abs(q.x) < 3000, abs(q.y) < 3000 else { started = false; continue }
                    if started { g.addLine(to: q) } else { g.move(to: q); started = true }
                }
                if started { g.addLine(to: cam.project(rider, ele: e) ?? .zero) }
                g.setStrokeColor(UIColor.orange.cgColor); g.setLineWidth(5); g.strokePath()
                if let hp = cam.project(rider, ele: e) {
                    let r = CGRect(x: hp.x - 9, y: hp.y - 9, width: 18, height: 18)
                    g.setFillColor(UIColor.orange.cgColor); g.fillEllipse(in: r)
                    g.setStrokeColor(UIColor.white.cgColor); g.setLineWidth(3); g.strokeEllipse(in: r)
                    if let k = photo { drawCard(k, above: hp) }
                }
            }
            while !input.isReadyForMoreMediaData { try? await Task.sleep(for: .milliseconds(5)) }
            ad.append(buffer(img, px), withPresentationTime: CMTime(value: CMTimeValue(f), timescale: CMTimeScale(fps)))
            if f % 30 == 0 { dlog(String(format: "[snapshot] frame %d/%d t=%.1f", f, n, t)) }
        }
        input.markAsFinished()
        await w.finishWriting()
        let bytes = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? Int) ?? -1
        let sorted = times.sorted()
        dlog(String(format: "[snapshot] device=%@ frames=%d failed=%d video=%.1fs snapshot median=%.0fms p90=%.0fms max=%.0fms wall=%.1fs bytes=%d",
                     UIDevice.current.model, times.count, failed, seconds, sorted[sorted.count / 2] * 1000, sorted[sorted.count * 9 / 10] * 1000,
                     sorted.last! * 1000, CACurrentMediaTime() - wall, bytes))
        dlog("SNAPSHOT DONE")
    }

    static func drawCard(_ k: Int, above p: CGPoint) {
        let card = CGRect(x: p.x - 70, y: p.y - 150, width: 140, height: 120)
        UIColor.white.withAlphaComponent(0.92).setFill()
        UIBezierPath(roundedRect: card, cornerRadius: 14).fill()
        UIColor.systemBlue.setFill()
        UIBezierPath(roundedRect: card.insetBy(dx: 8, dy: 8).divided(atDistance: 80, from: .minYEdge).slice, cornerRadius: 10).fill()
        let sym = UIImage(systemName: "photo.fill", withConfiguration: UIImage.SymbolConfiguration(pointSize: 40))!.withTintColor(.white)
        sym.draw(at: CGPoint(x: card.midX - sym.size.width / 2, y: card.minY + 48 - sym.size.height / 2))
        let text = "Photo \(k + 1)" as NSString
        let attrs: [NSAttributedString.Key: Any] = [.font: UIFont.boldSystemFont(ofSize: 13), .foregroundColor: UIColor.black]
        let ts = text.size(withAttributes: attrs)
        text.draw(at: CGPoint(x: card.midX - ts.width / 2, y: card.maxY - 26), withAttributes: attrs)
    }

    // Calibration dump: point(for:) of track points and a ground grid at several pitches, for fitting the camera model offline.
    static func calibDump(_ m: Flyover) async {
        var out: [[String: Any]] = []
        let places = [20.0, 12.0, 30.0]
        for t in places {
            let fi = m.track.index(at: t), c = m.track.coord(at: fi), h = m.track.heading(atTime: t)
            for p in [0.0, 35.0, 70.0] {
                for d in [1500.0, 3000.0] {
                    let o = MKMapSnapshotter.Options()
                    o.camera = MKMapCamera(lookingAtCenter: c, fromDistance: d, pitch: p, heading: h)
                    o.preferredConfiguration = MKImageryMapConfiguration(elevationStyle: .realistic)
                    o.size = CGSize(width: 393, height: 852)
                    guard let snap = try? await MKMapSnapshotter(options: o).start() else { continue }
                    var pts: [[Double]] = []
                    for dy in stride(from: -3000.0, through: 3000, by: 250) {
                        for dx in stride(from: -3000.0, through: 3000, by: 250) {
                            let q = CLLocationCoordinate2D(latitude: c.latitude + dy / 111_320, longitude: c.longitude + dx / (111_320 * cos(c.latitude * .pi / 180)))
                            let s = snap.point(for: q)
                            pts.append([q.latitude, q.longitude, s.x, s.y])
                        }
                    }
                    out.append(["t": t, "lat": c.latitude, "lon": c.longitude, "ele": m.track.ele[Int(fi)], "heading": h, "pitch": p, "dist": d, "w": 393, "h": 852, "pts": pts])
                    try? snap.image.pngData()?.write(to: docs.appendingPathComponent("calib-t\(Int(t))-p\(Int(p))-d\(Int(d)).png"))
                }
            }
        }
        let data = try! JSONSerialization.data(withJSONObject: out)
        try? data.write(to: docs.appendingPathComponent("calib.json"))
        print("[calib] done \(out.count) snapshots")
    }

    // Terrain-aware projection check: our projection (orange) versus point(for:) (cyan) on steep snapshots.
    static func projCheck(_ m: Flyover) async {
        for t in [8.0, 20.0, 29.0] {
            for (d, p) in [(1500.0, 60.0), (500.0, 70.0)] {
                let fi = m.track.index(at: t), c = m.track.coord(at: fi), h = m.track.heading(atTime: t)
                let size = CGSize(width: 393, height: 852)
                let o = MKMapSnapshotter.Options()
                o.camera = MKMapCamera(lookingAtCenter: c, fromDistance: d, pitch: p, heading: h)
                o.preferredConfiguration = MKImageryMapConfiguration(elevationStyle: .realistic)
                o.size = size
                o.traitCollection = UITraitCollection(displayScale: 3)
                guard let snap = try? await MKMapSnapshotter(options: o).start() else { continue }
                let cam = TerrainCamera(center: c, centerEle: m.track.ele[Int(fi)], distance: d, pitch: p, heading: h, size: size)
                let img = UIGraphicsImageRenderer(size: size).image { ctx in
                    snap.image.draw(at: .zero)
                    let g = ctx.cgContext
                    g.setLineWidth(1)
                    path(g, m.track.coords, snap); g.setStrokeColor(UIColor.cyan.cgColor); g.strokePath()
                    var started = false
                    for i in 0..<m.track.coords.count {
                        guard let q = cam.project(m.track.coords[i], ele: m.track.ele[i]), abs(q.x) < 3000, abs(q.y) < 3000 else { started = false; continue }
                        if started { g.addLine(to: q) } else { g.move(to: q); started = true }
                    }
                    g.setStrokeColor(UIColor.orange.cgColor); g.strokePath()
                }
                try? img.pngData()?.write(to: docs.appendingPathComponent("projcheck-t\(Int(t))-d\(Int(d))-p\(Int(p)).png"))
            }
        }
        print("[projcheck] done")
    }

    // Points behind the camera project to garbage; only a spike, so they are just dropped when far off-screen.
    static func path(_ g: CGContext, _ cs: [CLLocationCoordinate2D], _ s: MKMapSnapshotter.Snapshot) {
        var started = false
        for c in cs {
            let p = s.point(for: c)
            guard p.x.isFinite, p.y.isFinite, abs(p.x) < 5000, abs(p.y) < 5000 else { started = false; continue }
            if started { g.addLine(to: p) } else { g.move(to: p); started = true }
        }
    }

    static func buffer(_ img: UIImage, _ px: CGSize) -> CVPixelBuffer {
        var pb: CVPixelBuffer?
        CVPixelBufferCreate(nil, Int(px.width), Int(px.height), kCVPixelFormatType_32BGRA, nil, &pb)
        CVPixelBufferLockBaseAddress(pb!, [])
        let ctx = CGContext(data: CVPixelBufferGetBaseAddress(pb!), width: Int(px.width), height: Int(px.height), bitsPerComponent: 8,
                            bytesPerRow: CVPixelBufferGetBytesPerRow(pb!), space: CGColorSpaceCreateDeviceRGB(),
                            bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)!
        ctx.draw(img.cgImage!, in: CGRect(origin: .zero, size: px))
        CVPixelBufferUnlockBaseAddress(pb!, [])
        return pb!
    }
}
