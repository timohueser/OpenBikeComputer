import SwiftUI

// The drawn pieces of the launch/pairing screens (§4): the Bluetooth rune (SF
// Symbols ships no Bluetooth glyph), the little device illustration, and the
// scanning pulse rings. Kept together — they exist only for these screens.

/// The design's Bluetooth rune, traced from its 24×24 SVG path
/// (`M6.5 6.5 17 17l-5 5V2l5 5L6.5 17.5`), plus the optional disabled slash.
/// Stroke it like any shape; `strokeBorder`-free on purpose (it's an open path).
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

/// The hardware drawing: the two-tone shell (forest body seated on a celadon base that shows as a
/// lip), a white memory-LCD screen with the rust title bar, and the four side buttons — UP / DOWN
/// on the left flank, SELECT / BACK on the right. Two variants straight from the §4 frames.
struct DeviceGlyphView: View {
    enum Variant {
        /// A (104×128): named title bar + the amber track squiggle.
        case home(name: String)
        /// D1 (88×108): blank title bar + "PAIR" on screen.
        case pairing
    }

    let variant: Variant

    private var isHome: Bool {
        if case .home = variant { return true }
        return false
    }

    var body: some View {
        let shell: CGSize = isHome ? CGSize(width: 104, height: 128) : CGSize(width: 88, height: 108)
        let screen: CGSize = isHome ? CGSize(width: 80, height: 98) : CGSize(width: 66, height: 82)
        let radius: CGFloat = isHome ? 20 : 18
        let lip: CGFloat = isHome ? 3 : 2.5

        ZStack {
            // The celadon base peeking out below and to the sides of the body.
            RoundedRectangle(cornerRadius: radius + lip)
                .fill(OBCTheme.deviceAccent)
                .padding(.top, lip * 2)
                .padding(.horizontal, -lip)
                .padding(.bottom, -lip)

            RoundedRectangle(cornerRadius: radius)
                .fill(OBCTheme.deviceBody)
                .shadow(color: OBCTheme.deviceBody.opacity(0.3), radius: 13, y: isHome ? 14 : 0)

            screenContent
                .frame(width: screen.width, height: screen.height)
                .background(.white)
                .clipShape(RoundedRectangle(cornerRadius: isHome ? 8 : 7))
        }
        .frame(width: shell.width, height: shell.height)
        .overlay(alignment: .topLeading) { sideButtons.offset(x: -3, y: isHome ? 34 : 28) }
        .overlay(alignment: .topTrailing) { sideButtons.offset(x: 3, y: isHome ? 34 : 28) }
    }

    @ViewBuilder
    private var screenContent: some View {
        switch variant {
        case .home(let name):
            VStack(spacing: 0) {
                Text(name.uppercased())
                    .font(.obcMono(size: 8.5, weight: .bold))
                    .kerning(0.5)
                    .foregroundStyle(OBCTheme.deviceHeaderText)
                    .frame(maxWidth: .infinity)
                    .frame(height: 20)
                    .background(OBCTheme.deviceHeader)
                TrackSquiggle()
                    .stroke(OBCTheme.deviceTrack, style: StrokeStyle(lineWidth: 3.4, lineCap: .round))
                    .frame(maxHeight: .infinity)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 8)
            }
        case .pairing:
            VStack(spacing: 0) {
                OBCTheme.deviceHeader
                    .frame(height: 17)
                Text("PAIR")
                    .font(.obcMono(size: 10, weight: .bold))
                    .foregroundStyle(OBCTheme.deviceHeader)
                    .frame(maxHeight: .infinity)
            }
        }
    }

    /// One flank's pair of buttons — the same shape on both sides, so the device reads symmetric
    /// (UP / DOWN on the left, SELECT / BACK on the right).
    private var sideButtons: some View {
        VStack(spacing: isHome ? 9 : 8) {
            ForEach(0..<2, id: \.self) { _ in
                RoundedRectangle(cornerRadius: 2)
                    .fill(OBCTheme.deviceButton)
                    .frame(width: 5, height: isHome ? 15 : 13)
            }
        }
    }

    /// The little route line on the home screen (the §4 SVG path, normalized).
    private struct TrackSquiggle: Shape {
        func path(in rect: CGRect) -> Path {
            // Source path in an 80×74 box: M12 60 C 20 40 34 44 40 52 C 48 62 60 40 68 20
            func point(_ x: CGFloat, _ y: CGFloat) -> CGPoint {
                CGPoint(x: rect.minX + x / 80 * rect.width, y: rect.minY + y / 74 * rect.height)
            }
            var path = Path()
            path.move(to: point(12, 60))
            path.addCurve(to: point(40, 52), control1: point(20, 40), control2: point(34, 44))
            path.addCurve(to: point(68, 20), control1: point(48, 62), control2: point(60, 40))
            return path
        }
    }
}

/// D2's pulsing forest rings around the Bluetooth tile.
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
            .fill(OBCTheme.forest.opacity(0.22))
            .frame(width: 96, height: 96)
            .scaleEffect(animating ? 2.0 : 0.9)
            .opacity(animating ? 0 : 0.9)
            .animation(
                .easeOut(duration: 1.8).repeatForever(autoreverses: false).delay(delay),
                value: animating
            )
    }
}

/// The forest Bluetooth tile (D2 center, D3 backdrop).
struct BluetoothTile: View {
    var body: some View {
        RoundedRectangle(cornerRadius: 20)
            .fill(OBCTheme.forest)
            .frame(width: 72, height: 72)
            .overlay {
                BluetoothRune()
                    .stroke(.white, style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
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
            .stroke(OBCTheme.inkSoft, style: StrokeStyle(lineWidth: 1.9, lineCap: .round, lineJoin: .round))
            .frame(width: 36, height: 36)
    }
    .padding(30)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .background(OBCTheme.parchment)
}
