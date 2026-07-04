import SwiftUI

/// The tappable **disclosure row** on route detail (`.disc-row`) — the
/// "Waypoints · 4" entry. A standalone panel row: 30pt amber-tinted icon tile
/// (9pt radius), label, mono value, chevron.
///
/// Two behaviors: the plain init fires `action` (a push), the `isExpanded`
/// init folds `content` out below the row **inside the same panel** — the
/// waypoints dropdown on route detail. The chevron rotates to point down while
/// expanded.
public struct OBCDisclosureRow<Content: View>: View {
    let systemImage: String
    let label: String
    let value: String?
    private let isExpanded: Binding<Bool>?
    private let headerAccessibilityID: String?
    private let action: () -> Void
    private let content: Content

    /// Expanding variant: tapping the row folds `content` out below it.
    /// `headerAccessibilityID` lands on the header button (not the panel), so
    /// UI tests can keep tapping the row while the dropdown is open.
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
            .accessibilityIdentifier(headerAccessibilityID ?? "")

            if expandedNow {
                OBCTheme.screenLine
                    .frame(height: 1)
                    .padding(.horizontal, 16)
                content
                    .padding(.horizontal, 16)
                    .padding(.bottom, 8)
            }
        }
        .background(OBCTheme.panel)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
        .overlay(
            RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.line)
        )
    }

    private var header: some View {
        HStack(spacing: 12) {
            Image(systemName: systemImage)
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(OBCTheme.amber)
                .frame(width: 30, height: 30)
                .background(OBCTheme.amber.opacity(0.16))
                .clipShape(RoundedRectangle(cornerRadius: 9))

            Text(label)
                .font(.system(size: 16, weight: .medium))
                .foregroundStyle(OBCTheme.ink)
                .frame(maxWidth: .infinity, alignment: .leading)

            if let value {
                Text(value)
                    .font(.obcMono(size: 14))
                    .foregroundStyle(OBCTheme.inkFaint)
            }

            Image(systemName: "chevron.right")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(OBCTheme.inkFaint)
                .rotationEffect(.degrees(expandedNow ? 90 : 0))
                .padding(.leading, 10)
        }
        .padding(.vertical, 15)
        .padding(.horizontal, 16)
        .contentShape(Rectangle())
    }
}

extension OBCDisclosureRow where Content == EmptyView {
    /// Plain (push) variant: the whole row is a button firing `action`.
    public init(
        systemImage: String,
        label: String,
        value: String? = nil,
        action: @escaping () -> Void = {}
    ) {
        self.systemImage = systemImage
        self.label = label
        self.value = value
        self.isExpanded = nil
        self.headerAccessibilityID = nil
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
                    .font(.system(size: 14))
                    .foregroundStyle(OBCTheme.inkSoft)
                    .padding(.vertical, 12)
            }
        }
        .padding(20)
        .background(OBCTheme.parchment)
    }
}

#Preview("Disclosure row") {
    DisclosureRowPreviewHost()
}
#endif
