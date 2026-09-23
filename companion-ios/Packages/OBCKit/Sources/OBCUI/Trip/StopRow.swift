import SwiftUI
import OBCDomain

/// The mark of a stop's kind: a white symbol on the kind's colour. The same mark is the row
/// icon in the stops sheet, the pin on the map and the mark on the profile.
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
            .foregroundStyle(.white)
            .frame(width: size, height: size)
            .background(Self.color(kind))
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
    static func name(_ kind: Stop.Kind) -> String {
        switch kind {
        case .campsite: "Campsite"
        case .hotel: "Hotel"
        case .waypoint: "Waypoint"
        case .place: "Place"
        }
    }

    static func color(_ kind: Stop.Kind) -> Color {
        switch kind {
        case .campsite: OBCTheme.forest
        case .hotel: OBCTheme.water
        case .waypoint: OBCTheme.amber
        case .place: OBCTheme.inkSoft
        }
    }
}

/// One stop in the stops sheet: the kind's mark, the name, and a mono line under it.
public struct StopRow: View {
    let stop: Stop
    let detail: String
    let isEnabled: Bool
    let showsDivider: Bool
    let action: () -> Void

    public init(stop: Stop, detail: String, isEnabled: Bool = true, showsDivider: Bool = true, action: @escaping () -> Void) {
        self.stop = stop
        self.detail = detail
        self.isEnabled = isEnabled
        self.showsDivider = showsDivider
        self.action = action
    }

    public var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                StopIcon(kind: stop.kind)
                VStack(alignment: .leading, spacing: 2) {
                    Text(stop.name)
                        .font(.system(size: 16))
                        .foregroundStyle(OBCTheme.ink)
                        .lineLimit(1)
                    Text(detail)
                        .font(.obcMono(size: 12))
                        .foregroundStyle(OBCTheme.inkFaint)
                        .lineLimit(1)
                        .minimumScaleFactor(0.85)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.vertical, 11)
            .padding(.horizontal, 14)
            .frame(minHeight: 52)
            .contentShape(Rectangle())
            .overlay(alignment: .bottom) {
                if showsDivider { OBCTheme.screenLine.frame(height: 1).padding(.leading, 54) }
            }
        }
        .buttonStyle(.plain)
        .disabled(!isEnabled)
        .opacity(isEnabled ? 1 : 0.4)
    }
}
