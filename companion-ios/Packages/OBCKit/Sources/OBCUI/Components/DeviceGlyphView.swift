import SwiftUI

/// The hardware drawing: the two-tone shell, the black bezel around a white
/// memory-LCD screen, and the four side buttons. Every dimension derives from the
/// shell height through `Metrics`, off the same 308×470 body and 240×320 panel
/// proportions the simulator housing uses, so the glyph is the device in miniature.
struct DeviceGlyphView: View {
    enum Variant {
        /// Named title bar and the amber track squiggle.
        case home(name: String)
        /// Blank title bar and "PAIR" on screen. Drawn a little smaller.
        case pairing
        /// The device's route overview for a route it now holds. Drawn large, at two thirds of a
        /// point per panel pixel, so each panel pixel is whole screen pixels at 3x.
        case routeOverview(DeviceRouteOverview)
    }

    let variant: Variant

    /// The pairing glyph sits flat; the others cast a drop shadow.
    private var isRaised: Bool {
        if case .pairing = variant { return false }
        return true
    }

    private var glyphHeight: CGFloat {
        switch variant {
        case .home: 148
        case .pairing: 126
        case .routeOverview: 470 * 2 / 3
        }
    }

    /// The device's real proportions, scaled to a glyph height. Ratios are
    /// `dimension / 470`, the body height in the housing's screen-pixel units.
    private struct Metrics {
        let height: CGFloat

        var width: CGFloat { height * 308 / 470 }
        var radius: CGFloat { height * 42 / 470 }
        var lip: CGFloat { max(2, height * 6 / 470) }
        var screenWidth: CGFloat { height * 240 / 470 }
        var screenHeight: CGFloat { height * 320 / 470 }
        /// Screen top, measured from the body's top edge; the chin below is deeper.
        var screenTop: CGFloat { height * 32 / 470 }
        var bezelGap: CGFloat { height * 16 / 470 }
        var bezelRadius: CGFloat { height * 26 / 470 }
        var screenRadius: CGFloat { height * 10 / 470 }
        var buttonWidth: CGFloat { max(4, height * 19 / 470) }
        var buttonHeight: CGFloat { height * 66 / 470 }
        var buttonGap: CGFloat { height * 22 / 470 }
        var buttonProtrude: CGFloat { height * 13 / 470 }
        /// The wordmark's baseline inset from the body's bottom edge, centring it in the chin.
        var chinInset: CGFloat { height * 30 / 470 }
    }

    private var m: Metrics { Metrics(height: glyphHeight) }

    var body: some View {
        let m = self.m
        ZStack(alignment: .top) {
            // The celadon rim: the same slab grown evenly on all four sides.
            RoundedRectangle(cornerRadius: m.radius + m.lip)
                .fill(OBCTheme.deviceAccent)
                .padding(-m.lip)

            RoundedRectangle(cornerRadius: m.radius)
                .fill(OBCTheme.deviceBody)
                .shadow(color: OBCTheme.deviceBody.opacity(0.3), radius: 13, y: isRaised ? 14 : 0)

            // Bezel and screen, seated high so the wordmark chin reads below them.
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

            Text("OBC")
                .font(.system(size: m.width * 0.13, weight: .bold, design: .monospaced))
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
                    .font(.system(size: m.screenWidth * 0.113, weight: .bold, design: .monospaced))
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
                    .font(.system(size: m.screenWidth * 0.16, weight: .bold, design: .monospaced))
                    .foregroundStyle(OBCTheme.deviceHeader)
                    .frame(maxHeight: .infinity)
            }
        case .routeOverview(let overview):
            DeviceRouteOverviewScreen(overview: overview)
        }
    }

    /// One flank's pair of buttons. The `.leading`/`.trailing` overlay alignment
    /// centres the pair on the body's vertical midpoint, as on the hardware.
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

    /// The little route line on the home screen.
    private struct TrackSquiggle: Shape {
        func path(in rect: CGRect) -> Path {
            // The source path is "M12 60 C 20 40 34 44 40 52 C 48 62 60 40 68 20" in
            // an 80×74 box.
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
