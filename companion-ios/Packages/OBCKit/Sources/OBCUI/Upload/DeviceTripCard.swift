import SwiftUI
import OBCDomain

/// What the device's TRIP RECEIVED card shows for a trip it now holds.
public struct DeviceTripCard: Equatable, Sendable {
    public let name: String
    /// The device sums its day routes' whole kilometres and metres.
    public let distanceKm: Int
    public let climbMeters: Int
    public let dayCount: Int

    public init(name: String, days: [RouteSummary]) {
        self.name = name
        distanceKm = days.reduce(0) { $0 + Int($1.distanceMeters / 1000 + 0.5) }
        climbMeters = days.reduce(0) { $0 + Int($1.elevationGainMeters.rounded()) }
        dayCount = days.count
    }

    /// The name as the card's first line fits it: 15 cells of the body font.
    var line: String { String(name.prefix(15)) }
}

/// The device's TRIP RECEIVED card in its exact on-glass colours: the name, the summed stats, the
/// route count, and View trip selected over Dismiss. Drawn in the panel's own pixel coordinates,
/// taken from the firmware's layout.
struct DeviceTripCardScreen: View {
    let card: DeviceTripCard

    var body: some View {
        Canvas { context, size in
            context.deviceFrame(size: size, title: "TRIP RECEIVED")
            context.pixelText(card.line, .body, x: 120, capTop: 52, color: OBCTheme.deviceInk, centered: true)
            context.pixelText(
                "\(card.distanceKm) km, +\(card.climbMeters) m", .label, x: 120, capTop: 82,
                color: OBCTheme.deviceCaption, centered: true)
            context.pixelText(
                "\(card.dayCount) \(card.dayCount == 1 ? "route" : "routes")", .label, x: 120, capTop: 106,
                color: OBCTheme.deviceCaption, centered: true)
            context.fill(
                Path(roundedRect: CGRect(x: 12, y: 130, width: 216, height: 46), cornerRadius: 5),
                with: .color(OBCTheme.deviceTrack)
            )
            context.pixelText("View trip", .body, x: 28, capTop: 145, color: OBCTheme.deviceInk)
            context.pixelText("Dismiss", .body, x: 28, capTop: 199, color: OBCTheme.deviceInk)
        }
    }
}
