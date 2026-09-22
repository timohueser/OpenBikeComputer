import SwiftUI

/// The two wireframe candidates for the handle and its label. Delete after the owner's pick.
public enum MarkerHandleStyle: String, CaseIterable, Sendable {
    /// A neutral knob on the line; the label is a chip.
    case knob
    /// A pin in the colour of the day it ends, tip on the line; the label is bare text.
    case pin
}

/// The grab mark for one marker, drawn the same on the profile and on the map. `anchor` is the
/// point of the view that sits on the line.
struct MarkerHandleView: View {
    let style: MarkerHandleStyle
    let color: Color
    let isActive: Bool

    static func size(_ style: MarkerHandleStyle) -> CGSize {
        switch style {
        case .knob: CGSize(width: 24, height: 24)
        case .pin: CGSize(width: 24, height: 30)
        }
    }

    static func anchor(_ style: MarkerHandleStyle) -> UnitPoint {
        switch style {
        case .knob: .center
        case .pin: .bottom
        }
    }

    var body: some View {
        Group {
            switch style {
            case .knob:
                Circle()
                    .fill(OBCTheme.panel)
                    .overlay(Circle().strokeBorder(isActive ? OBCTheme.forest : OBCTheme.ink, lineWidth: isActive ? 2.5 : 2))
                    .frame(width: isActive ? 20 : 16, height: isActive ? 20 : 16)
                    .shadow(color: OBCTheme.ink.opacity(0.18), radius: isActive ? 4 : 1.5, y: 1)
            case .pin:
                PinShape()
                    .fill(color)
                    .overlay(PinShape().stroke(OBCTheme.panel, lineWidth: 2))
                    .frame(width: isActive ? 20 : 16, height: isActive ? 27 : 22)
                    .shadow(color: OBCTheme.ink.opacity(0.22), radius: isActive ? 4 : 1.5, y: 1)
            }
        }
        .frame(width: Self.size(style).width, height: Self.size(style).height, alignment: style == .pin ? .bottom : .center)
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
    let style: MarkerHandleStyle
    let text: String

    var body: some View {
        switch style {
        case .knob:
            Text(text)
                .font(.obcMono(size: 11, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
                .padding(.vertical, 4)
                .padding(.horizontal, 7)
                .background(OBCTheme.panel.opacity(0.94))
                .clipShape(RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(OBCTheme.line))
        case .pin:
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
}
