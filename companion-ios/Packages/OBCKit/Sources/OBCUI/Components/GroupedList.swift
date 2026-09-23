import SwiftUI

public struct OBCGroupedSection<Rows: View>: View {
    let header: String?
    let footer: String?
    @ViewBuilder let rows: Rows

    public init(_ header: String? = nil, footer: String? = nil, @ViewBuilder rows: () -> Rows) {
        self.header = header
        self.footer = footer
        self.rows = rows()
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let header {
                Text(header.uppercased())
                    .font(.system(size: 12.5, weight: .semibold))
                    .kerning(0.25)
                    .foregroundStyle(OBCTheme.inkFaint)
                    .padding(.horizontal, 8)
                    .padding(.bottom, 8)
            }

            VStack(spacing: 0) { rows }
                .background(OBCTheme.panel)
                .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
                .overlay(
                    RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.line)
                )

            if let footer {
                Text(footer)
                    .font(.system(size: 12.5))
                    .foregroundStyle(OBCTheme.inkFaint)
                    .padding(.horizontal, 10)
                    .padding(.top, 8)
            }
        }
    }
}

/// One grouped-list row. Chevron implies `action`; `value` renders trailing
/// faint; `comingSoon` badges and disables the row.
public struct OBCListRow<Trailing: View>: View {
    let icon: String?
    let iconColor: Color
    let label: String
    /// A second line under the label, in the mono stat face.
    let detail: String?
    /// Overrides the label's ink, for the warning-red "Forget device" row.
    let labelColor: Color?
    let value: String?
    var showsChevron: Bool
    var disabled: Bool
    var comingSoon: Bool
    var showsDivider: Bool
    let action: (() -> Void)?
    @ViewBuilder let trailing: Trailing

    public init(
        icon: String? = nil,
        iconColor: Color = OBCTheme.forest,
        label: String,
        detail: String? = nil,
        labelColor: Color? = nil,
        value: String? = nil,
        showsChevron: Bool = false,
        disabled: Bool = false,
        comingSoon: Bool = false,
        showsDivider: Bool = true,
        action: (() -> Void)? = nil,
        @ViewBuilder trailing: () -> Trailing
    ) {
        self.icon = icon
        self.iconColor = iconColor
        self.label = label
        self.detail = detail
        self.labelColor = labelColor
        self.value = value
        self.showsChevron = showsChevron
        self.disabled = disabled
        self.comingSoon = comingSoon
        self.showsDivider = showsDivider
        self.action = action
        self.trailing = trailing()
    }

    public var body: some View {
        let content = HStack(spacing: 12) {
            if let icon {
                OBCIconTile(systemImage: icon, color: iconColor)
            }
            VStack(alignment: .leading, spacing: 3) {
                Text(label)
                    .font(.system(size: 16))
                    .foregroundStyle(
                        disabled || comingSoon ? OBCTheme.inkFaint : labelColor ?? OBCTheme.ink)
                if let detail {
                    Text(detail)
                        .font(.obcMono(size: 12))
                        .foregroundStyle(OBCTheme.inkFaint)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if comingSoon {
                OBCSoonBadge()
            }
            if let value {
                Text(value)
                    .font(.system(size: 15))
                    .foregroundStyle(OBCTheme.inkFaint)
            }
            trailing
            if showsChevron {
                Image(systemName: "chevron.right")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(OBCTheme.inkFaint)
            }
        }
        .padding(.vertical, 14)
        .padding(.horizontal, 16)
        .frame(minHeight: 52)
        .overlay(alignment: .bottom) {
            if showsDivider {
                OBCTheme.screenLine.frame(height: 1).padding(.leading, icon == nil ? 16 : 56)
            }
        }
        .contentShape(Rectangle())

        if let action, !disabled, !comingSoon {
            Button(action: action) { content }.buttonStyle(.plain)
        } else {
            content
        }
    }
}

public extension OBCListRow where Trailing == EmptyView {
    /// Row without a custom trailing view. It lets `action` be the trailing closure at
    /// call sites without closure-matching ambiguity.
    init(
        icon: String? = nil,
        iconColor: Color = OBCTheme.forest,
        label: String,
        detail: String? = nil,
        labelColor: Color? = nil,
        value: String? = nil,
        showsChevron: Bool = false,
        disabled: Bool = false,
        comingSoon: Bool = false,
        showsDivider: Bool = true,
        action: (() -> Void)? = nil
    ) {
        self.init(
            icon: icon,
            iconColor: iconColor,
            label: label,
            detail: detail,
            labelColor: labelColor,
            value: value,
            showsChevron: showsChevron,
            disabled: disabled,
            comingSoon: comingSoon,
            showsDivider: showsDivider,
            action: action,
            trailing: { EmptyView() }
        )
    }
}

/// The tinted icon tile leading a settings row. `glyphColor` covers the neutral tiles,
/// where a white glyph would vanish.
public struct OBCIconTile: View {
    let systemImage: String
    let color: Color
    let glyphColor: Color

    public init(systemImage: String, color: Color, glyphColor: Color = .white) {
        self.systemImage = systemImage
        self.color = color
        self.glyphColor = glyphColor
    }

    public var body: some View {
        Image(systemName: systemImage)
            .font(.system(size: 14, weight: .medium))
            .foregroundStyle(glyphColor)
            .frame(width: 28, height: 28)
            .background(color)
            .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusSmall))
    }
}

/// The amber-outline "COMING SOON" badge.
public struct OBCSoonBadge: View {
    let text: String

    public init(_ text: String = "Coming soon") { self.text = text }

    public var body: some View {
        Text(text.uppercased())
            .font(.obcMono(size: 9.5, weight: .bold))
            .kerning(0.75)
            .foregroundStyle(OBCTheme.amber)
            .padding(.vertical, 4)
            .padding(.horizontal, 6)
            .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(OBCTheme.amber))
    }
}

#Preview("Grouped list") {
    ScrollView {
        VStack(spacing: 26) {
            OBCGroupedSection("Device", footer: "Renaming updates the name shown on the device at the next sync.") {
                OBCListRow(icon: "pencil", iconColor: OBCTheme.forest, label: "Name", value: "Trailhead", showsChevron: true) {}
                OBCListRow(icon: "arrow.triangle.2.circlepath", iconColor: OBCTheme.wood, label: "Firmware update", comingSoon: true)
                OBCListRow(icon: "xmark.circle", iconColor: OBCTheme.warning, label: "Forget this device", showsDivider: false) {}
            }
            OBCGroupedSection("Connected services") {
                OBCListRow(icon: "bolt", iconColor: OBCTheme.coral, label: "Strava", comingSoon: true)
                OBCListRow(icon: "dot.radiowaves.left.and.right", iconColor: OBCTheme.wood, label: "Komoot", comingSoon: true, showsDivider: false)
            }
        }
        .padding(20)
    }
    .background(OBCTheme.parchment)
}
