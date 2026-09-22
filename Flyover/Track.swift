import Foundation
import CoreLocation

// Minimal GPX parse -> resample to a fixed step -> pacing timeline with photo holds.

struct RawPoint { var lat, lon, ele: Double }

final class GPXParser: NSObject, XMLParserDelegate {
    var points: [RawPoint] = []
    private var cur: RawPoint?
    private var text = ""

    static func parse(_ url: URL) -> [RawPoint] {
        let p = GPXParser()
        let x = XMLParser(contentsOf: url)!
        x.delegate = p
        x.parse()
        return p.points
    }

    func parser(_ parser: XMLParser, didStartElement el: String, namespaceURI: String?, qualifiedName: String?, attributes a: [String: String] = [:]) {
        text = ""
        if el == "trkpt", let la = Double(a["lat"] ?? ""), let lo = Double(a["lon"] ?? "") {
            cur = RawPoint(lat: la, lon: lo, ele: 0)
        }
    }
    func parser(_ parser: XMLParser, foundCharacters s: String) { text += s }
    func parser(_ parser: XMLParser, didEndElement el: String, namespaceURI: String?, qualifiedName: String?) {
        if el == "ele" { cur?.ele = Double(text.trimmingCharacters(in: .whitespacesAndNewlines)) ?? 0 }
        if el == "trkpt", let c = cur { points.append(c); cur = nil }
    }
}

func haversine(_ a: CLLocationCoordinate2D, _ b: CLLocationCoordinate2D) -> Double {
    let r = 6_371_000.0
    let dLat = (b.latitude - a.latitude) * .pi / 180, dLon = (b.longitude - a.longitude) * .pi / 180
    let h = sin(dLat / 2) * sin(dLat / 2) + cos(a.latitude * .pi / 180) * cos(b.latitude * .pi / 180) * sin(dLon / 2) * sin(dLon / 2)
    return 2 * r * asin(min(1, sqrt(h)))
}

func bearing(_ a: CLLocationCoordinate2D, _ b: CLLocationCoordinate2D) -> Double {
    let la1 = a.latitude * .pi / 180, la2 = b.latitude * .pi / 180, dLon = (b.longitude - a.longitude) * .pi / 180
    let y = sin(dLon) * cos(la2), x = cos(la1) * sin(la2) - sin(la1) * cos(la2) * cos(dLon)
    return atan2(y, x) * 180 / .pi
}

func offset(_ c: CLLocationCoordinate2D, meters: Double, heading: Double) -> CLLocationCoordinate2D {
    let h = heading * .pi / 180
    let dn = meters * cos(h), de = meters * sin(h)
    return CLLocationCoordinate2D(latitude: c.latitude + dn / 111_320,
                                  longitude: c.longitude + de / (111_320 * cos(c.latitude * .pi / 180)))
}

struct Track {
    static let step = 10.0 // meters between resampled points

    let coords: [CLLocationCoordinate2D]
    let ele: [Double]
    private(set) var headingT: [Double] = [] // camera heading at 30 Hz of timeline time, unwrapped
    let length: Double

    // Pacing timeline: piecewise-linear (time -> sample index as Double). A photo hold is two knots with the same index.
    let knotT: [Double]
    let knotI: [Double]
    let photos: [(index: Int, holdStart: Double, holdEnd: Double)]
    var duration: Double { knotT.last! }

    init(raw: [RawPoint], rideSeconds: Double = 40, holdSeconds: Double = 2, climbWeight: Double = 10) {
        // Resample at a fixed distance step.
        var cs: [CLLocationCoordinate2D] = [], es: [Double] = []
        var carry = 0.0
        for k in 1..<raw.count {
            let a = CLLocationCoordinate2D(latitude: raw[k - 1].lat, longitude: raw[k - 1].lon)
            let b = CLLocationCoordinate2D(latitude: raw[k].lat, longitude: raw[k].lon)
            let seg = haversine(a, b)
            var s = carry
            while s < seg {
                let f = s / seg
                cs.append(.init(latitude: a.latitude + (b.latitude - a.latitude) * f, longitude: a.longitude + (b.longitude - a.longitude) * f))
                es.append(raw[k - 1].ele + (raw[k].ele - raw[k - 1].ele) * f)
                s += Track.step
            }
            carry = s - seg
        }
        coords = cs
        length = Double(cs.count - 1) * Track.step

        // Smooth elevation (±150 m) so pacing does not jitter on noisy DEM steps.
        let n = cs.count
        var sm = [Double](repeating: 0, count: n)
        for i in 0..<n {
            let lo = max(0, i - 15), hi = min(n - 1, i + 15)
            sm[i] = es[lo...hi].reduce(0, +) / Double(hi - lo + 1)
        }
        ele = sm

        // Pacing: each segment's cost = 1 + climbWeight * uphill grade, scaled to rideSeconds total.
        // Grade is averaged over ±1.5 km: a short-window grade makes the playback speed pulse.
        var grade = [Double](repeating: 0, count: n)
        for i in 0..<n {
            let lo = max(0, i - 150), hi = min(n - 1, i + 150)
            grade[i] = max(0, (sm[hi] - sm[lo]) / (Double(hi - lo) * Track.step))
        }
        var cost = [Double](repeating: 0, count: n)
        for i in 1..<n { cost[i] = cost[i - 1] + 1 + climbWeight * grade[i] }
        let scale = rideSeconds / cost[n - 1]
        let photoIdx = [0.25, 0.5, 0.75].map { Int(Double(n - 1) * $0) }
        var kt: [Double] = [], ki: [Double] = [], ph: [(Int, Double, Double)] = []
        var shift = 0.0
        for i in 0..<n {
            kt.append(cost[i] * scale + shift); ki.append(Double(i))
            if photoIdx.contains(i) {
                let s = kt.last!
                shift += holdSeconds
                kt.append(s + holdSeconds); ki.append(Double(i))
                ph.append((i, s, s + holdSeconds))
            }
        }
        knotT = kt; knotI = ki; photos = ph

        // Camera heading in the time domain: aim 300 m ahead, zero-phase low-pass (τ≈1 s), then cap the turn rate.
        // Averaging bearings over distance fails at out-and-back turnarounds: the vectors cancel and the heading flips.
        let hz = 30.0, frames = Int(duration * hz) + 2
        var target = [Double](repeating: 0, count: frames), prev = 0.0
        for f in 0..<frames {
            let fi = index(at: Double(f) / hz)
            var h = bearing(coord(at: fi), coord(at: min(Double(n - 1), fi + 30)))
            if f > 0 { while h - prev > 180 { h -= 360 }; while h - prev < -180 { h += 360 } }
            target[f] = h; prev = h
        }
        let a = 1 / (1.0 * hz)
        var y = target
        for f in 1..<frames { y[f] = y[f - 1] + a * (target[f] - y[f - 1]) }
        for f in stride(from: frames - 2, through: 0, by: -1) { y[f] = y[f + 1] + a * (y[f] - y[f + 1]) }
        let maxStep = 40.0 / hz // deg per frame
        for f in 1..<frames { y[f] = y[f - 1] + max(-maxStep, min(maxStep, y[f] - y[f - 1])) }
        headingT = y
    }

    func heading(atTime t: Double) -> Double {
        let x = max(0, t) * 30, i = min(Int(x), headingT.count - 2), f = x - Double(i)
        return headingT[i] + (headingT[i + 1] - headingT[i]) * f
    }

    /// Fractional sample index at timeline time t.
    func index(at t: Double) -> Double {
        if t <= 0 { return 0 }
        if t >= duration { return Double(coords.count - 1) }
        var lo = 0, hi = knotT.count - 1
        while hi - lo > 1 { let m = (lo + hi) / 2; if knotT[m] <= t { lo = m } else { hi = m } }
        let span = knotT[hi] - knotT[lo]
        return span <= 0 ? knotI[lo] : knotI[lo] + (knotI[hi] - knotI[lo]) * (t - knotT[lo]) / span
    }

    func coord(at fi: Double) -> CLLocationCoordinate2D {
        let i = min(Int(fi), coords.count - 2), f = fi - Double(i)
        let a = coords[i], b = coords[i + 1]
        return .init(latitude: a.latitude + (b.latitude - a.latitude) * f, longitude: a.longitude + (b.longitude - a.longitude) * f)
    }

}
