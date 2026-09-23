import SwiftUI
import OBCDomain

/// The quiet row under a ride's stats line: "Merge with Day 2 Ulrichen (2)?". The row merges
/// after a confirmation; ✕ dismisses it for good. `load` finds the next ride once, off the
/// first frame.
public struct RideMergeSuggestion: View {
    private let load: @MainActor () async -> RideSummary?
    private let onMerge: () -> Void
    private let onDismiss: () -> Void
    @State private var next: RideSummary?
    @State private var confirmShown = false

    public init(
        load: @escaping @MainActor () async -> RideSummary?,
        onMerge: @escaping () -> Void,
        onDismiss: @escaping () -> Void
    ) {
        self.load = load
        self.onMerge = onMerge
        self.onDismiss = onDismiss
    }

    public var body: some View {
        // A stack, not a Group: an empty Group is no view, and its task would never run.
        VStack(spacing: 0) {
            if let next {
                HStack(spacing: 10) {
                    Button { confirmShown = true } label: {
                        HStack(spacing: 10) {
                            Image(systemName: "arrow.triangle.merge")
                                .font(.system(size: 15, weight: .medium))
                                .foregroundStyle(OBCTheme.forest)
                            Text("Merge with \(next.name)?")
                                .font(.system(size: 16))
                                .foregroundStyle(OBCTheme.ink)
                                .lineLimit(1)
                            Spacer(minLength: 0)
                            Image(systemName: "chevron.right")
                                .font(.system(size: 13, weight: .semibold))
                                .foregroundStyle(OBCTheme.inkFaint)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("detail.mergeSuggestion")
                    Button {
                        self.next = nil
                        onDismiss()
                    } label: {
                        Image(systemName: "xmark")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(OBCTheme.inkFaint)
                            .frame(width: 32, height: 32)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Dismiss")
                    .accessibilityIdentifier("detail.mergeSuggestion.dismiss")
                }
                .padding(.vertical, 6)
                .overlay(alignment: .top) { Rectangle().fill(OBCTheme.line).frame(height: 1) }
                .overlay(alignment: .bottom) { Rectangle().fill(OBCTheme.line).frame(height: 1) }
                .rideMergeConfirmation(next: next, isPresented: $confirmShown, onMerge: onMerge)
            }
        }
        .task { next = await load() }
    }
}
