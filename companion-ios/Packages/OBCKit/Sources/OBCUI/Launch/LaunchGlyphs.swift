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

/// The hardware drawing: the two-tone shell (forest body inside a concentric celadon rim), the
/// black bezel around a white memory-LCD screen with the rust title bar, and the four side buttons
/// — UP / DOWN on the left flank, SELECT / BACK on the right, the pair centred on the body's
/// midpoint as on the real device.
///
/// Every dimension is derived from the shell **height** through [`Metrics`], off the same 308×470
/// body / 240×320 panel proportions the simulator housing uses — so the glyph is the real device
/// in miniature (≈0.65 w:h, a tall chin under the bezel) rather than a rounded square.
struct DeviceGlyphView: View {
    enum Variant {
        /// A: named title bar + the amber track squiggle.
        case home(name: String)
        /// D1: blank title bar + "PAIR" on screen. Drawn a little smaller.
        case pairing
    }

    let variant: Variant

    private var isHome: Bool {
        if case .home = variant { return true }
        return false
    }

    /// The device's real proportions, scaled to a given glyph height. Ratios are `dimension / 470`
    /// (the body height in the housing's screen-pixel units).
    private struct Metrics {
        let height: CGFloat

        var width: CGFloat { height * 308 / 470 }
        var radius: CGFloat { height * 42 / 470 }
        /// The celadon rim, even on all four sides.
        var lip: CGFloat { max(2, height * 6 / 470) }
        var screenWidth: CGFloat { height * 240 / 470 }
        var screenHeight: CGFloat { height * 320 / 470 }
        /// Screen top, measured from the body's top edge — the chin below is much deeper.
        var screenTop: CGFloat { height * 32 / 470 }
        var bezelGap: CGFloat { height * 16 / 470 }
        var bezelRadius: CGFloat { height * 26 / 470 }
        var screenRadius: CGFloat { height * 10 / 470 }
        var buttonWidth: CGFloat { max(4, height * 19 / 470) }
        var buttonHeight: CGFloat { height * 54 / 470 }
        var buttonGap: CGFloat { height * 26 / 470 }
        /// How far a pad protrudes past the body edge.
        var buttonProtrude: CGFloat { height * 13 / 470 }
        /// The wordmark's baseline inset from the body's bottom edge, centring it in the chin.
        var chinInset: CGFloat { height * 30 / 470 }
    }

    private var m: Metrics { Metrics(height: isHome ? 148 : 126) }

    var body: some View {
        let m = self.m
        ZStack(alignment: .top) {
            // The celadon rim: the same slab grown evenly on all four sides.
            RoundedRectangle(cornerRadius: m.radius + m.lip)
                .fill(OBCTheme.deviceAccent)
                .padding(-m.lip)

            RoundedRectangle(cornerRadius: m.radius)
                .fill(OBCTheme.deviceBody)
                .shadow(color: OBCTheme.deviceBody.opacity(0.3), radius: 13, y: isHome ? 14 : 0)

            // Bezel + screen, seated high in the body so the wordmark chin reads below them.
            RoundedRectangle(cornerRadius: m.bezelRadius)
                .fill(OBCTheme.deviceBezel)
                .frame(width: m.screenWidth + 2 * m.bezelGap, height: m.screenHeight + 2 * m.bezelGap)
                .overlay {
                    screenContent
                        .frame(width: m.screenWidth, height: m.screenHeight)
                        .background(.white)
                        .clipShape(RoundedRectangle(cornerRadius: m.screenRadius))
                }
                .padding(.top, m.screenTop - m.bezelGap)

            // The wordmark embossed into the chin, as on the real shell.
            Text("OBC")
                .font(.obcMono(size: m.width * 0.13, weight: .bold))
                .kerning(m.width * 0.05)
                .foregroundStyle(OBCTheme.deviceAccent.opacity(0.28))
                .frame(maxHeight: .infinity, alignment: .bottom)
                .padding(.bottom, m.chinInset)
        }
        .frame(width: m.width, height: m.height)
        .overlay(alignment: .leading) { sideButtons.offset(x: -m.buttonProtrude) }
        .overlay(alignment: .trailing) { sideButtons.offset(x: m.buttonProtrude) }
    }

    @ViewBuilder
    private var screenContent: some View {
        let m = self.m
        switch variant {
        case .home(let name):
            VStack(spacing: 0) {
                Text(name.uppercased())
                    .font(.obcMono(size: m.screenWidth * 0.113, weight: .bold))
                    .kerning(0.5)
                    .minimumScaleFactor(0.7)
                    .lineLimit(1)
                    .foregroundStyle(OBCTheme.deviceHeaderText)
                    .frame(maxWidth: .infinity)
                    .frame(height: m.screenHeight * 0.2)
                    .background(OBCTheme.deviceHeader)
                TrackSquiggle()
                    .stroke(OBCTheme.deviceTrack, style: StrokeStyle(lineWidth: 3.4, lineCap: .round))
                    .frame(maxHeight: .infinity)
                    .padding(.horizontal, m.screenWidth * 0.08)
                    .padding(.vertical, m.screenHeight * 0.08)
            }
        case .pairing:
            VStack(spacing: 0) {
                OBCTheme.deviceHeader
                    .frame(height: m.screenHeight * 0.2)
                Text("PAIR")
                    .font(.obcMono(size: m.screenWidth * 0.16, weight: .bold))
                    .foregroundStyle(OBCTheme.deviceHeader)
                    .frame(maxHeight: .infinity)
            }
        }
    }

    /// One flank's pair of buttons — the same shape on both sides, so the device reads symmetric
    /// (UP / DOWN on the left, SELECT / BACK on the right). The `.leading`/`.trailing` overlay
    /// alignment centres the pair on the body's vertical midpoint, matching the hardware.
    private var sideButtons: some View {
        let m = self.m
        return VStack(spacing: m.buttonGap) {
            ForEach(0..<2, id: \.self) { _ in
                RoundedRectangle(cornerRadius: 2)
                    .fill(OBCTheme.deviceButton)
                    .frame(width: m.buttonWidth, height: m.buttonHeight)
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
