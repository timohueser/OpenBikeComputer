import SwiftUI

// The drawn pieces of the launch and pairing screens: the Bluetooth rune (SF Symbols
// ships no Bluetooth glyph) and the scanning pulse rings. The device illustration is
// `DeviceGlyphView`.

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

/// The pulsing rust rings around the Bluetooth tile.
struct PulsingRings: View {
    @State private var animating = false

    var body: some View {
        ZStack {
            ring(delay: 0)
            ring(delay: 0.9)
        }
        .onAppear { animating = true }
    }

    private func ring(delay: Double) -> some View {
        Circle()
            .fill(OBCTheme.rust.opacity(0.22))
            .frame(width: 96, height: 96)
            .scaleEffect(animating ? 2.0 : 0.9)
            .opacity(animating ? 0 : 0.9)
            .animation(
                .easeOut(duration: 1.8).repeatForever(autoreverses: false).delay(delay),
                value: animating
            )
    }
}

struct BluetoothTile: View {
    var body: some View {
        RoundedRectangle(cornerRadius: 20)
            .fill(OBCTheme.rust)
            .frame(width: 72, height: 72)
            .overlay {
                BluetoothRune()
                    .stroke(OBCTheme.onRust, style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                    .frame(width: 30, height: 30)
            }
    }
}

#Preview("Launch glyphs") {
    VStack(spacing: 30) {
        DeviceGlyphView(variant: .home(name: "Trailhead"))
        DeviceGlyphView(variant: .pairing)
        ZStack {
            PulsingRings()
            BluetoothTile()
        }
        .frame(width: 200, height: 200)
        BluetoothRune(slashed: true)
            .stroke(OBCTheme.secondary, style: StrokeStyle(lineWidth: 1.9, lineCap: .round, lineJoin: .round))
            .frame(width: 36, height: 36)
    }
    .padding(30)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .background(OBCTheme.page)
}
