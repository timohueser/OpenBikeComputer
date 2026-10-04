import SwiftUI
import OBCDomain

/// The mark of a stop's kind: the kind's symbol in the surface colour on ink, never amber, which is
/// the marker handle's. The same mark is the pin on the map and the mark on the profile.
public struct StopIcon: View {
    let kind: Stop.Kind
    var size: CGFloat = 28
    /// A circle for a pin or a profile mark; a rounded square for a row.
    var isRound = false

    public init(kind: Stop.Kind, size: CGFloat = 28, isRound: Bool = false) {
        self.kind = kind
        self.size = size
        self.isRound = isRound
    }

    public var body: some View {
        Image(systemName: Self.symbol(kind))
            .font(.system(size: size * 0.48, weight: .semibold))
            .foregroundStyle(OBCTheme.surface)
            .frame(width: size, height: size)
            .background(OBCTheme.ink)
            .clipShape(RoundedRectangle(cornerRadius: isRound ? size / 2 : OBCTheme.radiusSmall))
    }

    static func symbol(_ kind: Stop.Kind) -> String {
        switch kind {
        case .campsite: "tent.fill"
        case .hotel: "bed.double.fill"
        case .waypoint: "flag.fill"
        case .place: "mappin"
        }
    }

    /// The kind as a word: "Hotel".
    nonisolated static func name(_ kind: Stop.Kind) -> String {
        switch kind {
        case .campsite: "Campsite"
        case .hotel: "Hotel"
        case .waypoint: "Waypoint"
        case .place: "Place"
        }
    }
}
