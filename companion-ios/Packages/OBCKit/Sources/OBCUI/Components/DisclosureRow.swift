import SwiftUI

/// The tappable disclosure row on route detail, such as "Waypoints · 4".
/// Two behaviors: the plain init fires `action` as a push, and the `isExpanded` init
/// folds `content` out below the row inside the same panel.
public struct OBCDisclosureRow<Content: View>: View {
    let systemImage: String
    let label: String
    let value: String?
    private let isExpanded: Binding<Bool>?
    private let headerAccessibilityID: String?
    private let action: () -> Void
    private let content: Content

    /// Expanding variant: tapping the row folds `content` out below it.
    /// `headerAccessibilityID` lands on the header button, not the panel, so UI tests
    /// can keep tapping the row while the dropdown is open.
    public init(
        systemImage: String,
        label: String,
        value: String? = nil,
        isExpanded: Binding<Bool>,
        headerAccessibilityID: String? = nil,
        @ViewBuilder content: () -> Content
    ) {
        self.systemImage = systemImage
        self.label = label
        self.value = value
        self.isExpanded = isExpanded
        self.headerAccessibilityID = headerAccessibilityID
        self.action = {}
        self.content = content()
    }

    private var expandedNow: Bool { isExpanded?.wrappedValue == true }

    public var body: some View {
        VStack(spacing: 0) {
            Button {
                if let isExpanded {
                    withAnimation(.easeInOut(duration: 0.2)) {
                        isExpanded.wrappedValue.toggle()
                    }
                } else {
                    action()
                }
            } label: {
                header
            }
            .buttonStyle(.plain)
            .accessibilityLabel(label)
            .accessibilityValue([value, isExpanded.map { $0.wrappedValue ? "Expanded" : "Collapsed" }]
                .compactMap { $0 }.joined(separator: ", "))
            .accessibilityIdentifier(headerAccessibilityID ?? "")

            if expandedNow {
                OBCTheme.hairline
                    .frame(height: 1)
                    .padding(.horizontal, 16)
                content
                    .padding(.horizontal, 16)
                    .padding(.bottom, 8)
            }
        }
        .background(OBCTheme.surface)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
        .overlay(
            RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.hairline)
        )
    }

    private var header: some View {
        HStack(spacing: 12) {
            Image(systemName: systemImage)
                .font(.system(.subheadline, weight: .medium))
                .foregroundStyle(OBCTheme.secondary)
                .frame(width: 30, height: 30)
                .background(OBCTheme.fill)
                .clipShape(RoundedRectangle(cornerRadius: 9))

            Text(label)
                .font(.system(.callout, weight: .medium))
                .foregroundStyle(OBCTheme.ink)
                .frame(maxWidth: .infinity, alignment: .leading)

            if let value {
                Text(value)
                    .font(.system(.subheadline).monospacedDigit())
                    .foregroundStyle(OBCTheme.secondary)
            }

            Image(systemName: isExpanded == nil ? "chevron.right" : "chevron.down")
                .font(.system(.footnote, weight: .semibold))
                .foregroundStyle(OBCTheme.secondary)
                .rotationEffect(.degrees(expandedNow ? 180 : 0))
                .padding(.leading, 10)
        }
        .padding(.vertical, 15)
        .padding(.horizontal, 16)
        .contentShape(Rectangle())
    }
}

extension OBCDisclosureRow where Content == EmptyView {
    /// Plain push variant: the whole row is a button firing `action`.
    /// `accessibilityID` lands on that button, so UI tests can tap the row.
    public init(
        systemImage: String,
        label: String,
        value: String? = nil,
        accessibilityID: String? = nil,
        action: @escaping () -> Void = {}
    ) {
        self.systemImage = systemImage
        self.label = label
        self.value = value
        self.isExpanded = nil
        self.headerAccessibilityID = accessibilityID
        self.action = action
        self.content = EmptyView()
    }
}

#if DEBUG
private struct DisclosureRowPreviewHost: View {
    @State private var expanded = true

    var body: some View {
        VStack(spacing: 14) {
            OBCDisclosureRow(systemImage: "mappin.and.ellipse", label: "Waypoints", value: "4")
            OBCDisclosureRow(
                systemImage: "mappin.and.ellipse",
                label: "Waypoints",
                value: "2",
                isExpanded: $expanded
            ) {
                Text("Dropdown content")
                    .font(.system(.subheadline))
                    .foregroundStyle(OBCTheme.secondary)
                    .padding(.vertical, 12)
            }
        }
        .padding(20)
        .background(OBCTheme.page)
    }
}

#Preview("Disclosure row") {
    DisclosureRowPreviewHost()
}
#endif
