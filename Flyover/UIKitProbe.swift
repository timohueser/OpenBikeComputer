import SwiftUI
import MapKit

// Probe: does UIKit MKMapView accept a higher pitch than SwiftUI Map?
struct UIKitProbe: UIViewRepresentable {
    let center: CLLocationCoordinate2D, dist: Double, pitch: Double, heading: Double
    func makeUIView(context: Context) -> MKMapView {
        let v = MKMapView()
        switch UserDefaults.standard.string(forKey: "cfg") ?? "" {
        case "stdflat": v.preferredConfiguration = MKStandardMapConfiguration(elevationStyle: .flat)
        case "std3d": v.preferredConfiguration = MKStandardMapConfiguration(elevationStyle: .realistic)
        case "hyb3d": v.preferredConfiguration = MKHybridMapConfiguration(elevationStyle: .realistic)
        case "imgflat": v.preferredConfiguration = MKImageryMapConfiguration(elevationStyle: .flat)
        default: v.preferredConfiguration = MKImageryMapConfiguration(elevationStyle: .realistic)
        }
        v.isPitchEnabled = true
        v.camera = MKMapCamera(lookingAtCenter: center, fromDistance: dist, pitch: pitch, heading: heading)
        for delay in [0.5, 3, 8] {
            DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
                print(String(format: "[uikit %.1fs] pitch=%.1f dist=%.0f alt=%.0f", delay, v.camera.pitch, v.camera.centerCoordinateDistance, v.camera.altitude))
                if delay == 3 { v.camera = MKMapCamera(lookingAtCenter: center, fromDistance: dist, pitch: pitch, heading: heading) }
            }
        }
        return v
    }
    func updateUIView(_ v: MKMapView, context: Context) {}
}

// UIKit variant of the per-frame driver: MKMapView camera set every display frame, trail = one full polyline
// whose renderer's strokeEnd grows (no overlay churn).
struct UIKitFlyover: UIViewRepresentable {
    let m: Flyover
    func makeCoordinator() -> Coord { Coord() }
    func makeUIView(context: Context) -> MKMapView {
        let v = MKMapView()
        v.preferredConfiguration = MKImageryMapConfiguration(elevationStyle: .realistic)
        let c = context.coordinator
        v.delegate = c
        let all = MKPolyline(coordinates: m.track.coords, count: m.track.coords.count)
        c.full = all
        v.addOverlay(MKPolyline(coordinates: m.track.coords, count: m.track.coords.count))
        v.addOverlay(all)
        v.addAnnotation(c.head)
        v.showsCompass = false
        m.mapView = v
        for p in m.track.photos { let a = MKPointAnnotation(); a.coordinate = m.track.coords[p.index]; v.addAnnotation(a) }
        v.camera = cam(m.camera(at: m.t))
        m.onFrame = { [weak v] in
            guard let v else { return }
            let fi = m.track.index(at: m.t)
            v.camera = cam(m.camera(at: m.t))
            c.trail?.strokeEnd = fi / Double(m.track.coords.count - 1)
            c.head.coordinate = m.track.coord(at: fi)
        }
        // A camera set before the view has a size loses its pitch; apply it again after layout.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { m.onFrame?() }
        return v
    }
    func updateUIView(_ v: MKMapView, context: Context) {}
    func cam(_ c: MapCamera) -> MKMapCamera {
        MKMapCamera(lookingAtCenter: c.centerCoordinate, fromDistance: c.distance, pitch: c.pitch, heading: c.heading)
    }

    final class Coord: NSObject, MKMapViewDelegate {
        var full: MKPolyline?
        var trail: MKPolylineRenderer?
        let head = MKPointAnnotation()
        func mapView(_ v: MKMapView, rendererFor o: MKOverlay) -> MKOverlayRenderer {
            let r = MKPolylineRenderer(polyline: o as! MKPolyline)
            if o === full {
                r.strokeColor = .orange; r.lineWidth = 5; r.strokeEnd = 0; trail = r
            } else {
                r.strokeColor = .white.withAlphaComponent(0.45); r.lineWidth = 3
            }
            return r
        }
        func mapView(_ v: MKMapView, viewFor a: MKAnnotation) -> MKAnnotationView? {
            if a !== head {
                let mv = MKMarkerAnnotationView(annotation: a, reuseIdentifier: "photo")
                mv.glyphImage = UIImage(systemName: "camera.fill"); mv.markerTintColor = .systemBlue
                return mv
            }
            let av = MKAnnotationView(annotation: a, reuseIdentifier: "head")
            let dot = UIView(frame: .init(x: 0, y: 0, width: 18, height: 18))
            dot.backgroundColor = .orange; dot.layer.cornerRadius = 9; dot.layer.borderWidth = 3; dot.layer.borderColor = UIColor.white.cgColor
            av.addSubview(dot); av.frame = dot.frame
            return av
        }
    }
}
