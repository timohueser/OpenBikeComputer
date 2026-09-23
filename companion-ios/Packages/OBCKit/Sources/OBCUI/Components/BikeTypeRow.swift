import SwiftUI
import OBCDomain

/// The Bike type row: the current type, and a menu of the four types. Route detail and the trip
/// page use it inside their grouped sections.
public struct OBCBikeTypeRow: View {
    let type: BikeType
    let showsDivider: Bool
    let onChange: (BikeType) -> Void

    public init(type: BikeType, showsDivider: Bool = false, onChange: @escaping (BikeType) -> Void) {
        self.type = type
        self.showsDivider = showsDivider
        self.onChange = onChange
    }

    public var body: some View {
        Menu {
            Picker("Bike type", selection: Binding(get: { type }, set: onChange)) {
                ForEach(BikeType.allCases, id: \.self) { Text($0.name).tag($0) }
            }
        } label: {
            OBCListRow(label: "Bike type", value: type.name, showsDivider: showsDivider) {
                Image(systemName: "chevron.up.chevron.down")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(OBCTheme.inkFaint)
            }
        }
        .buttonStyle(.plain)
    }
}
