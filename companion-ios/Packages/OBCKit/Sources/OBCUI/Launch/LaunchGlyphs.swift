import SwiftUI

// The drawn pieces of the launch and pairing screens: the device's pairing card, the
// Bluetooth rune (SF Symbols ships no Bluetooth glyph), the scanning rings and the radio
// states. The device illustration is `DeviceGlyphView`.

/// The device's pairing card, from the firmware's passkey screen: the title bar, the
/// device-and-phone glyph, the code and its two caption lines, in panel pixels. The app never
/// learns the code, so the six digits are drawn as blanks.
struct DevicePasskeyScreen: View {
    var body: some View {
        Canvas { context, size in
            context.deviceFrame(size: size, title: "PAIRING")
            drawPairGlyph(in: &context, cx: 120, cy: 67)
            context.pixelText("------", .huge, x: 120, capTop: 114, color: OBCTheme.deviceInk, centered: true)
            context.pixelText("Enter this code", .label, x: 120, capTop: 178, color: OBCTheme.deviceCaption, centered: true)
            context.pixelText("on your phone", .label, x: 120, capTop: 202, color: OBCTheme.deviceCaption, centered: true)
        }
    }

    /// The Bluetooth rune, three link dashes and a phone outline, 60 px wide, centred on `cx`.
    private func drawPairGlyph(in context: inout GraphicsContext, cx: CGFloat, cy: CGFloat) {
        let x0 = cx - 30
        let (stem, tip, left) = (x0 + 3, x0 + 10, x0)
        let (top, bottom, quarter) = (cy - 8, cy + 8, CGFloat(4))
        let strokes: [((CGFloat, CGFloat), (CGFloat, CGFloat))] = [
            ((stem, top), (stem, bottom)), ((stem, top), (tip, top + quarter)), ((tip, top + quarter), (stem, cy)),
            ((stem, bottom), (tip, bottom - quarter)), ((tip, bottom - quarter), (stem, cy)),
            ((tip, top + quarter), (left, bottom - quarter)), ((tip, bottom - quarter), (left, top + quarter)),
        ]
        var rune = Path()
        for (a, b) in strokes {
            rune.move(to: CGPoint(x: a.0 + 0.5, y: a.1 + 0.5))
            rune.addLine(to: CGPoint(x: b.0 + 0.5, y: b.1 + 0.5))
        }
        let ink = GraphicsContext.Shading.color(OBCTheme.deviceInk)
        context.stroke(rune, with: ink, lineWidth: 1)
        for index in 0..<3 {
            context.fill(Path(CGRect(x: x0 + 19 + CGFloat(index) * 8, y: cy - 1, width: 5, height: 2)), with: ink)
        }
        let phone = CGRect(x: x0 + 48, y: cy - 10, width: 12, height: 20)
        context.stroke(Path(roundedRect: phone.insetBy(dx: 1, dy: 1), cornerRadius: 2.5), with: ink, lineWidth: 2)
        context.fill(Path(CGRect(x: phone.minX + 4, y: cy - 6, width: 4, height: 1)), with: ink)
    }
}

/// The Bluetooth rune, traced from its 24×24 SVG path
/// (`M6.5 6.5 17 17l-5 5V2l5 5L6.5 17.5`), plus the optional disabled slash.
/// It is an open path, so stroke it; `strokeBorder` does not apply.
struct BluetoothRune: Shape {
    var slashed = false

    func path(in rect: CGRect) -> Path {
        // Map the 24×24 design grid onto rect.
        let s = min(rect.width, rect.height) / 24
        let dx = rect.midX - 12 * s
        let dy = rect.midY - 12 * s
        func point(_ x: CGFloat, _ y: CGFloat) -> CGPoint {
            CGPoint(x: dx + x * s, y: dy + y * s)
        }
        var path = Path()
        path.move(to: point(6.5, 6.5))
        path.addLine(to: point(17, 17))
        path.addLine(to: point(12, 22))
        path.addLine(to: point(12, 2))
        path.addLine(to: point(17, 7))
        path.addLine(to: point(6.5, 17.5))
        if slashed {
            path.move(to: point(2, 2))
            path.addLine(to: point(22, 22))
        }
        return path
    }
}

/// The rune in a surface tile: the phone's Bluetooth, looking for the device.
struct BluetoothTile: View {
    var body: some View {
        RoundedRectangle(cornerRadius: 20)
            .fill(OBCTheme.surface)
            .frame(width: 72, height: 72)
            .overlay {
                BluetoothRune()
                    .stroke(OBCTheme.ink, style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                    .frame(width: 30, height: 30)
            }
            .accessibilityHidden(true)
    }
}

/// The scanning rings around the Bluetooth tile. With Reduce Motion they hold still.
struct PulsingRings: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var animating = false

    var body: some View {
        ZStack {
            if reduceMotion {
                ring.scaleEffect(1.7).opacity(0.5)
                ring.scaleEffect(1.3)
            } else {
                pulse(delay: 0)
                pulse(delay: 0.9)
            }
        }
        .onAppear { animating = true }
        .accessibilityHidden(true)
    }

    private var ring: some View {
        Circle()
            .fill(OBCTheme.secondary.opacity(0.14))
            .frame(width: 96, height: 96)
    }

    private func pulse(delay: Double) -> some View {
        ring
            .scaleEffect(animating ? 2.0 : 0.9)
            .opacity(animating ? 0 : 1)
            .animation(
                .easeOut(duration: 1.8).repeatForever(autoreverses: false).delay(delay),
                value: animating
            )
    }
}

/// The glyph of a blocked radio. Off draws the phone's Bluetooth switch turned off, so the screen
/// says which switch to turn on; denied draws the rune under a lock.
struct RadioBlockedGlyph: View {
    let block: LaunchFlowModel.RadioBlock

    var body: some View {
        HStack(spacing: 14) {
            BluetoothRune()
                .stroke(OBCTheme.ink, style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                .frame(width: 34, height: 34)
            switch block {
            case .off:
                Capsule()
                    .fill(OBCTheme.fill)
                    .frame(width: 64, height: 38)
                    .overlay(alignment: .leading) {
                        Circle()
                            .fill(OBCTheme.surface)
                            .shadow(color: OBCTheme.ink.opacity(0.18), radius: 2, y: 1)
                            .padding(3)
                    }
            case .denied:
                Image(systemName: "lock.fill")
                    .font(.system(size: 26, weight: .semibold))
                    .foregroundStyle(OBCTheme.secondary)
            }
        }
        .padding(.horizontal, 22)
        .padding(.vertical, 18)
        .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusLarge))
        .accessibilityHidden(true)
    }
}

/// The slashed rune on a tinted disc: the link did not come up.
struct NoLinkMark: View {
    let tint: Color

    var body: some View {
        Circle()
            .fill(tint.opacity(0.1))
            .frame(width: 88, height: 88)
            .overlay {
                BluetoothRune(slashed: true)
                    .stroke(tint, style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                    .frame(width: 40, height: 40)
            }
            .accessibilityHidden(true)
    }
}

#Preview("Launch glyphs") {
    ScrollView {
        VStack(spacing: 30) {
            DeviceGlyphView(variant: .home(name: "Trailhead"))
            DeviceGlyphView(variant: .passkey)
            ZStack {
                PulsingRings()
                BluetoothTile()
            }
            .frame(width: 200, height: 200)
            RadioBlockedGlyph(block: .off)
            RadioBlockedGlyph(block: .denied)
            NoLinkMark(tint: OBCTheme.danger)
        }
        .padding(30)
        .frame(maxWidth: .infinity)
    }
    .background(OBCTheme.page)
}
