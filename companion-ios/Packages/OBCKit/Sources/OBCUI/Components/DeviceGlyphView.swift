import SwiftUI

/// The hardware drawing: the two-tone shell, the black bezel around a white
/// memory-LCD screen, and the four side buttons. Every dimension derives from the
/// shell height through `Metrics`, off the same 308×470 body and 240×320 panel
/// proportions the simulator housing uses, so the glyph is the device in miniature.
/// The screen draws a device page in its exact on-glass colours and Terminus.
struct DeviceGlyphView: View {
    enum Variant {
        /// The title bar with the device's name and the amber track squiggle.
        case home(name: String)
        /// The device's pairing card. The app never knows the code, so the digits are blanks.
        case passkey
        /// The device's route overview for a route it now holds.
        case routeOverview(DeviceRouteOverview)
        /// The card the device shows when a trip lands.
        case tripCard(DeviceTripCard)
        /// A card of the device's firmware-update flow.
        case firmware(DeviceFirmwareCard)
    }

    let variant: Variant

    /// Two thirds of a point per panel pixel, so each panel pixel is whole screen pixels at 3x.
    private static let glyphHeight: CGFloat = 470 * 2 / 3

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

    var body: some View {
        let m = Metrics(height: Self.glyphHeight)
        ZStack(alignment: .top) {
            // The celadon rim: the same slab grown evenly on all four sides.
            RoundedRectangle(cornerRadius: m.radius + m.lip)
                .fill(OBCTheme.deviceAccent)
                .padding(-m.lip)

            RoundedRectangle(cornerRadius: m.radius)
                .fill(OBCTheme.deviceBody)
                .shadow(color: OBCTheme.deviceBody.opacity(0.3), radius: 13, y: 14)

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
        .overlay(alignment: .leading) { sideButtons(m).offset(x: -m.buttonProtrude) }
        .overlay(alignment: .trailing) { sideButtons(m).offset(x: m.buttonProtrude) }
        .accessibilityElement(children: .ignore)
        .accessibilityAddTraits(.isImage)
        .accessibilityLabel(accessibilityText)
    }

    @ViewBuilder
    private var screenContent: some View {
        switch variant {
        case .home(let name):
            DeviceHomeScreen(name: name)
        case .passkey:
            DevicePasskeyScreen()
        case .routeOverview(let overview):
            DeviceRouteOverviewScreen(overview: overview)
        case .tripCard(let card):
            DeviceTripCardScreen(card: card)
        case .firmware(let card):
            DeviceFirmwareScreen(card: card)
        }
    }

    private var accessibilityText: String {
        switch variant {
        case .home(let name): "\(name), the bike computer"
        case .passkey: "The bike computer's pairing screen, which shows a six-digit code"
        case .routeOverview(let overview): "The bike computer's screen showing \(overview.name)"
        case .tripCard(let card): "The bike computer's screen showing \(card.name)"
        case .firmware(let card): card.accessibilityText
        }
    }

    /// One flank's pair of buttons. The `.leading`/`.trailing` overlay alignment
    /// centres the pair on the body's vertical midpoint, as on the hardware.
    private func sideButtons(_ m: Metrics) -> some View {
        VStack(spacing: m.buttonGap) {
            ForEach(0..<2, id: \.self) { _ in
                RoundedRectangle(cornerRadius: 2)
                    .fill(OBCTheme.deviceButton)
                    .frame(width: m.buttonWidth, height: m.buttonHeight)
            }
        }
    }
}

/// The name in the title bar over the amber track squiggle.
private struct DeviceHomeScreen: View {
    let name: String

    var body: some View {
        Canvas { context, size in
            let title = name.count <= 15 ? name : String(name.prefix(13)).trimmingCharacters(in: .whitespaces) + ".."
            context.deviceFrame(size: size, title: title)
            // The source path is "M12 60 C 20 40 34 44 40 52 C 48 62 60 40 68 20" in an 80×74 box.
            let box = CGRect(x: 24, y: 70, width: 192, height: 200)
            func point(_ x: CGFloat, _ y: CGFloat) -> CGPoint {
                CGPoint(x: box.minX + x / 80 * box.width, y: box.minY + y / 74 * box.height)
            }
            var path = Path()
            path.move(to: point(12, 60))
            path.addCurve(to: point(40, 52), control1: point(20, 40), control2: point(34, 44))
            path.addCurve(to: point(68, 20), control1: point(48, 62), control2: point(60, 40))
            context.stroke(path, with: .color(OBCTheme.deviceTrack), style: StrokeStyle(lineWidth: 6, lineCap: .round))
        }
    }
}
