import SwiftUI

/// The grab mark for one marker, drawn the same on the profile and on the map: a pin in the
/// colour of the day it ends, with its tip on the line. A fixed marker's pin is faded.
struct MarkerHandleView: View {
    let color: Color
    let isActive: Bool
    var isFixed = false

    /// The view's frame; the tip sits at the bottom centre.
    static let size = CGSize(width: 24, height: 30)
    /// How far the pin rises above the finger while it is held. Tuned on a real phone.
    static let dragLift: CGFloat = 28

    var body: some View {
        PinShape()
            .fill(color)
            .overlay(PinShape().stroke(OBCTheme.panel, lineWidth: 2))
            .frame(width: isActive ? 20 : 16, height: isActive ? 27 : 22)
            .shadow(color: OBCTheme.ink.opacity(0.22), radius: isActive ? 4 : 1.5, y: 1)
            .opacity(isFixed ? 0.45 : 1)
            .frame(width: Self.size.width, height: Self.size.height, alignment: .bottom)
            .animation(.snappy(duration: 0.16), value: isActive)
    }
}

/// A teardrop: a round head over a point at the bottom centre.
private struct PinShape: Shape {
    func path(in rect: CGRect) -> Path {
        let r = rect.width / 2
        let center = CGPoint(x: rect.midX, y: rect.minY + r)
        var path = Path()
        // The head arc opens at the bottom, where the two tangents meet at the tip.
        path.addArc(center: center, radius: r, startAngle: .degrees(150), endAngle: .degrees(30), clockwise: false)
        path.addLine(to: CGPoint(x: rect.midX, y: rect.maxY))
        path.closeSubpath()
        return path
    }
}

/// The active marker's readout, "km 156 · 1,480 m", above its handle.
struct MarkerLabel: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.obcMono(size: 11, weight: .bold))
            .foregroundStyle(OBCTheme.ink)
            // A parchment halo instead of a box: four hard shadows keep it legible over tiles.
            .shadow(color: OBCTheme.panel, radius: 0, x: 1, y: 0)
            .shadow(color: OBCTheme.panel, radius: 0, x: -1, y: 0)
            .shadow(color: OBCTheme.panel, radius: 0, x: 0, y: 1)
            .shadow(color: OBCTheme.panel, radius: 0, x: 0, y: -1)
            .shadow(color: OBCTheme.panel, radius: 2)
    }
}
