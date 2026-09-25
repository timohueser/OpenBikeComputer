import SwiftUI
import OBCDomain

/// The confirmation after a send: the device drawn with the screen it now shows, "‹name› is on
/// ‹device›", where to find it, and Done. A route shows its overview; a trip shows the card the
/// device puts up when a trip lands.
public struct SentToDeviceView: View {
    private let screen: DeviceGlyphView.Variant
    private let name: String
    private let whereLine: String
    let deviceName: String
    let onDone: () -> Void

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var shown = false

    public init(overview: DeviceRouteOverview, deviceName: String, onDone: @escaping () -> Void) {
        screen = .routeOverview(overview)
        name = overview.name
        whereLine = "It is under Routes on the device."
        self.deviceName = deviceName
        self.onDone = onDone
    }

    public init(trip: DeviceTripCard, deviceName: String, onDone: @escaping () -> Void) {
        screen = .tripCard(trip)
        name = trip.name
        whereLine = trip.dayCount == 1
            ? "It is under Routes on the device." : "Its \(trip.dayCount) days are under Routes on the device."
        self.deviceName = deviceName
        self.onDone = onDone
    }

    public var body: some View {
        VStack(spacing: 0) {
            DeviceGlyphView(variant: screen)
                .padding(.top, 4)
                .padding(.bottom, 24)
                .scaleEffect(shown || reduceMotion ? 1 : 0.94)
                .opacity(shown || reduceMotion ? 1 : 0)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("\(deviceName) screen showing \(name)")
                .accessibilityAddTraits(.isImage)
                .accessibilityIdentifier("upload.deviceScreen")

            Text("\(name) is on \(deviceName)")
                .font(.system(.title2, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
                .multilineTextAlignment(.center)
                .accessibilityAddTraits(.isHeader)
                .accessibilityIdentifier("upload.doneTitle")
            Text(whereLine)
                .font(.system(.body))
                .foregroundStyle(OBCTheme.secondary)
                .multilineTextAlignment(.center)
                .padding(.top, 6)
                .padding(.bottom, 24)

            Button("Done", action: onDone)
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("upload.done")
        }
        .frame(maxWidth: .infinity)
        .onAppear {
            withAnimation(.spring(duration: 0.45, bounce: 0.2)) { shown = true }
        }
    }
}

#if DEBUG
#Preview("Sent to device") {
    SentToDeviceView(
        overview: DeviceRouteOverview(
            name: "Grimsel Pass", distanceMeters: 18_700, estimatedDuration: 4_740, track: .obcSample
        ),
        deviceName: "Trailhead",
        onDone: {}
    )
    .padding(22)
    .background(OBCTheme.surface)
}
#endif
