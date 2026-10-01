#if os(iOS)
import SwiftUI

enum PlannerPreviewDrawerPosition {
    case collapsed, open, expanded
    /// The header band, and the lowest detent. The host scales the same value to size its content.
    static let collapsedBase: CGFloat = 60
}

/// The native sheet owns dragging. Map layout only changes when a detent changes.
struct PlannerPreviewDrawer<Header: View, Content: View>: View {
    @Binding var position: PlannerPreviewDrawerPosition
    let openHeight: CGFloat
    var expandedHeight: CGFloat? = nil
    let onHeight: (CGFloat) -> Void
    @ViewBuilder let header: () -> Header
    @ViewBuilder let content: () -> Content
    @ScaledMetric(relativeTo: .body) private var collapsedHeight = PlannerPreviewDrawerPosition.collapsedBase

    var body: some View {
        VStack(spacing: 0) {
            header()
                .padding(.horizontal, 16)
                .padding(.top, 12)
                .padding(.bottom, 4)
                .frame(height: collapsedHeight)
            content()
                .frame(height: contentHeight - collapsedHeight, alignment: .top)
                .accessibilityHidden(position == .collapsed)
                .allowsHitTesting(position != .collapsed)
        }
        .frame(height: contentHeight, alignment: .top)
        .frame(minWidth: 0, maxWidth: .infinity, minHeight: 0, maxHeight: .infinity, alignment: .top)
        .clipped()
        .foregroundStyle(OBCTheme.ink)
        .tint(OBCTheme.tint)
        .presentationDetents(detents, selection: detent)
        .presentationDragIndicator(.visible)
        .presentationCornerRadius(OBCTheme.radiusSheet)
        .presentationBackground(OBCTheme.surface2)
        .presentationBackgroundInteraction(.enabled)
        .presentationContentInteraction(.scrolls)
        .interactiveDismissDisabled()
        .onAppear { onHeight(targetHeight) }
        .onChange(of: targetHeight) { _, height in onHeight(height) }
        .onChange(of: expandedDetentHeight, initial: true) { _, height in
            if height == nil && position == .expanded { position = .open }
        }
        .accessibilityElement(children: .contain)
        .accessibilityActions {
            Button("Collapse route panel") { position = .collapsed }
            Button("Show planning controls") { position = .open }
            if expandedDetentHeight != nil {
                Button("Expand elevation profile") { position = .expanded }
            }
        }
    }

    private var openDetentHeight: CGFloat { max(openHeight, collapsedHeight + 80) }
    private var expandedDetentHeight: CGFloat? {
        guard let expandedHeight, expandedHeight > openDetentHeight else { return nil }
        return expandedHeight
    }
    private var contentHeight: CGFloat { position == .collapsed ? openDetentHeight : targetHeight }
    private var targetHeight: CGFloat {
        switch position {
        case .collapsed: collapsedHeight
        case .open: openDetentHeight
        case .expanded: expandedDetentHeight ?? openDetentHeight
        }
    }
    private var detents: Set<PresentationDetent> {
        var result: Set<PresentationDetent> = [.height(collapsedHeight), .height(openDetentHeight)]
        if let expandedDetentHeight { result.insert(.height(expandedDetentHeight)) }
        return result
    }
    private var detent: Binding<PresentationDetent> {
        Binding {
            .height(targetHeight)
        } set: { newValue in
            if newValue == .height(collapsedHeight) { position = .collapsed }
            else if let expandedDetentHeight, newValue == .height(expandedDetentHeight) { position = .expanded }
            else { position = .open }
        }
    }
}
#endif
