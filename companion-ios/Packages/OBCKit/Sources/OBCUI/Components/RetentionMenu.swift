import SwiftUI
import OBCDomain

/// The **Auto-delete** control (epic #638 S7) — a pull-down menu of the six
/// retention levels, shared by the Settings default row, the upload sheet, and
/// the route-detail control so every surface reads and edits the level
/// identically. The generic ``OBCRetentionMenu`` wraps any label; ``OBCRetentionRow``
/// is the field-guide row the three surfaces use (icon tile · label · value ·
/// menu chevron), with an optional secondary line for the detail's device expiry.

/// A pull-down menu of the six retention levels over an arbitrary `label`. The
/// options render in ``Retention/allCases`` order (Never → 2 months, the design's
/// list), the current one checked.
public struct OBCRetentionMenu<MenuLabel: View>: View {
    private let selection: Retention
    private let onSelect: (Retention) -> Void
    @ViewBuilder private let label: () -> MenuLabel

    public init(
        selection: Retention,
        onSelect: @escaping (Retention) -> Void,
        @ViewBuilder label: @escaping () -> MenuLabel
    ) {
        self.selection = selection
        self.onSelect = onSelect
        self.label = label
    }

    public var body: some View {
        Menu {
            ForEach(Retention.allCases, id: \.self) { option in
                Button {
                    onSelect(option)
                } label: {
                    if option == selection {
                        Label(OBCFormat.retentionLabel(option), systemImage: "checkmark")
                    } else {
                        Text(OBCFormat.retentionLabel(option))
                    }
                }
            }
        } label: {
            label()
        }
    }
}

/// The Auto-delete field row: a tinted icon tile, a label, the current level's
/// value, and the pull-down chevron — the whole row opens the menu. `detailLine`
/// renders faint under the label (the detail's "Expires in 2 days" device-truth
/// line; `nil` elsewhere). Sits inside an ``OBCGroupedSection`` (Settings, detail)
/// which supplies the panel + border.
public struct OBCRetentionRow: View {
    let icon: String
    let iconColor: Color
    let label: String
    let selection: Retention
    let detailLine: String?
    var showsDivider: Bool
    let accessibilityID: String
    let onSelect: (Retention) -> Void

    public init(
        icon: String = "trash",
        iconColor: Color = OBCTheme.wood,
        label: String = "Auto-delete",
        selection: Retention,
        detailLine: String? = nil,
        showsDivider: Bool = false,
        accessibilityID: String,
        onSelect: @escaping (Retention) -> Void
    ) {
        self.icon = icon
        self.iconColor = iconColor
        self.label = label
        self.selection = selection
        self.detailLine = detailLine
        self.showsDivider = showsDivider
        self.accessibilityID = accessibilityID
        self.onSelect = onSelect
    }

    public var body: some View {
        OBCRetentionMenu(selection: selection, onSelect: onSelect) {
            HStack(spacing: 12) {
                OBCIconTile(systemImage: icon, color: iconColor)
                VStack(alignment: .leading, spacing: 2) {
                    Text(label)
                        .font(.system(size: 16))
                        .foregroundStyle(OBCTheme.ink)
                    if let detailLine {
                        Text(detailLine)
                            .font(.obcMono(size: 12))
                            .foregroundStyle(OBCTheme.inkFaint)
                            .accessibilityIdentifier("\(accessibilityID).expiry")
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Text(OBCFormat.retentionLabel(selection))
                    .font(.system(size: 15))
                    .foregroundStyle(OBCTheme.inkSoft)
                    .accessibilityIdentifier("\(accessibilityID).value")
                Image(systemName: "chevron.up.chevron.down")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(OBCTheme.inkFaint)
            }
            .padding(.vertical, 14)
            .padding(.horizontal, 16)
            .frame(minHeight: 52)
            .overlay(alignment: .bottom) {
                if showsDivider {
                    OBCTheme.screenLine.frame(height: 1).padding(.leading, 56)
                }
            }
            .contentShape(Rectangle())
        }
        .accessibilityIdentifier(accessibilityID)
    }
}

#if DEBUG
#Preview("Auto-delete rows") {
    struct Host: View {
        @State private var setting: Retention = .twoWeeks
        @State private var route: Retention = .oneWeek
        var body: some View {
            ScrollView {
                VStack(spacing: 26) {
                    OBCGroupedSection("Routes", footer: "New routes you upload will auto-delete after this long.") {
                        OBCRetentionRow(
                            icon: "trash", iconColor: OBCTheme.wood,
                            label: "Auto-delete new routes",
                            selection: setting, accessibilityID: "settings.autoDelete"
                        ) { setting = $0 }
                    }
                    OBCGroupedSection {
                        OBCRetentionRow(
                            selection: route,
                            detailLine: "Expires in 2 days",
                            accessibilityID: "detail.autoDelete"
                        ) { route = $0 }
                    }
                }
                .padding(20)
            }
            .background(OBCTheme.parchment)
        }
    }
    return Host()
}
#endif
