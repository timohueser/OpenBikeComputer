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
                    .font(.system(.caption, weight: .semibold))
                    .kerning(0.25)
                    .foregroundStyle(OBCTheme.secondary)
                    .padding(.horizontal, 8)
                    .padding(.bottom, 8)
            }

            VStack(spacing: 0) { rows }
                .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
                .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusCard))

            if let footer {
                Text(footer)
                    .font(.system(.caption))
                    .foregroundStyle(OBCTheme.secondary)
                    .padding(.horizontal, 10)
                    .padding(.top, 8)
            }
        }
    }
}

/// One grouped-list row. Chevron implies `action`; `value` renders trailing
/// faint.
public struct OBCListRow<Trailing: View>: View {
    let icon: String?
    let iconColor: Color
    let label: String
    /// A second line under the label, in the stat face.
    let detail: String?
    /// Overrides the label's ink, for the danger-red "Forget device" row.
    let labelColor: Color?
    let value: String?
    var showsChevron: Bool
    var disabled: Bool
    var showsDivider: Bool
    let action: (() -> Void)?
    @ViewBuilder let trailing: Trailing

    public init(
        icon: String? = nil,
        iconColor: Color = OBCTheme.tint,
        label: String,
        detail: String? = nil,
        labelColor: Color? = nil,
        value: String? = nil,
        showsChevron: Bool = false,
        disabled: Bool = false,
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
                    .font(.system(.callout))
                    .foregroundStyle(
                        disabled ? OBCTheme.secondary : labelColor ?? OBCTheme.ink)
                if let detail {
                    Text(detail)
                        .font(.system(.caption).monospacedDigit())
                        .foregroundStyle(OBCTheme.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if let value {
                Text(value)
                    .font(.system(.subheadline))
                    .foregroundStyle(OBCTheme.secondary)
            }
            trailing
            if showsChevron {
                Image(systemName: "chevron.right")
                    .font(.system(.footnote, weight: .semibold))
                    .foregroundStyle(OBCTheme.secondary)
            }
        }
        .padding(.vertical, 14)
        .padding(.horizontal, 16)
        .frame(minHeight: 52)
        .overlay(alignment: .bottom) {
            if showsDivider {
                OBCTheme.hairline.frame(height: 1).padding(.leading, icon == nil ? 16 : 56)
            }
        }
        .contentShape(Rectangle())

        if let action, !disabled {
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
        iconColor: Color = OBCTheme.tint,
        label: String,
        detail: String? = nil,
        labelColor: Color? = nil,
        value: String? = nil,
        showsChevron: Bool = false,
        disabled: Bool = false,
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
            showsDivider: showsDivider,
            action: action,
            trailing: { EmptyView() }
        )
    }
}

/// The tinted icon tile leading a settings row: a surface-coloured glyph on `color`.
public struct OBCIconTile: View {
    let systemImage: String
    let color: Color

    public init(systemImage: String, color: Color) {
        self.systemImage = systemImage
        self.color = color
    }

    public var body: some View {
        Image(systemName: systemImage)
            .font(.system(.subheadline, weight: .medium))
            .foregroundStyle(OBCTheme.surface)
            .frame(width: 28, height: 28)
            .background(color)
            .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusSmall))
            .obcFixedGeometryType()
    }
}

/// The "COMING SOON" badge: a secondary caption in a hairline outline.
public struct OBCSoonBadge: View {
    let text: String

    public init(_ text: String = "Coming soon") { self.text = text }

    public var body: some View {
        Text(text.uppercased())
            .font(.system(.caption2, weight: .semibold).monospacedDigit())
            .kerning(0.75)
            .foregroundStyle(OBCTheme.secondary)
            .padding(.vertical, 4)
            .padding(.horizontal, 6)
            .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(OBCTheme.hairlineStrong))
    }
}

#Preview("Grouped list") {
    ScrollView {
        VStack(spacing: 26) {
            OBCGroupedSection("Device", footer: "Renaming updates the name shown on the device at the next sync.") {
                OBCListRow(icon: "pencil", iconColor: OBCTheme.tint, label: "Name", value: "Trailhead", showsChevron: true) {}
                OBCListRow(icon: "xmark.circle", iconColor: OBCTheme.danger, label: "Forget this device", showsDivider: false) {}
            }
        }
        .padding(20)
    }
    .background(OBCTheme.page)
}
