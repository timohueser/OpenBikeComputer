import MapKit

// MapKit's snapshot camera, fitted from snapshot.point(for:) (rms 0.1 px): vertical FOV 30°, eye at `distance` from the
// look-at point along `pitch`/`heading`, principal point at the image centre. point(for:) projects onto the flat plane through
// the look-at point; this adds each point's own height (GPX elevation minus the look-at point's ground elevation).
struct TerrainCamera {
    var center: CLLocationCoordinate2D
    var centerEle: Double
    var distance, pitch, heading: Double
    var size: CGSize

    func project(_ c: CLLocationCoordinate2D, ele: Double) -> CGPoint? {
        let r = 6_371_000.0, lat0 = center.latitude * .pi / 180
        let e = (c.longitude - center.longitude) * .pi / 180 * r * cos(lat0)
        let n = (c.latitude - center.latitude) * .pi / 180 * r
        let u = ele - centerEle - (e * e + n * n) / (2 * r)
        let p = pitch * .pi / 180, h = heading * .pi / 180
        let (sh, ch, sp, cp) = (sin(h), cos(h), sin(p), cos(p))
        // Camera position relative to the look-at point, and its basis.
        let cx = -sh * distance * sp, cy = -ch * distance * sp, cz = distance * cp
        let vx = e - cx, vy = n - cy, vz = u - cz
        let fwd = vx * sh * sp + vy * ch * sp - vz * cp
        guard fwd > 1 else { return nil }
        let right = vx * ch - vy * sh
        let up = vx * sh * cp + vy * ch * cp + vz * sp // r × f
        let f = size.height / 2 / tan(15 * .pi / 180)
        return CGPoint(x: size.width / 2 + right / fwd * f, y: size.height / 2 - up / fwd * f)
    }
}
