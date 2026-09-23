import SwiftUI
import OBCDomain

/// The quiet row under a ride's stats line: "Merge with Day 2 Ulrichen (2)?". A tap expands the
/// row in place with the next ride's start and length, and Merge and Cancel; ✕ dismisses it for
/// good. `load` finds the next ride once, off the
/// first frame.
public struct RideMergeSuggestion: View {
    private let load: @MainActor () async -> RideSummary?
    private let onMerge: () -> Void
    private let onDismiss: () -> Void
    @State private var next: RideSummary?
    @State private var expanded = false

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
                VStack(alignment: .leading, spacing: 10) {
                    HStack(spacing: 10) {
                        Button { expanded = true } label: {
                            HStack(spacing: 10) {
                                Image(systemName: "arrow.triangle.merge")
                                    .font(.system(size: 15, weight: .medium))
                                    .foregroundStyle(OBCTheme.forest)
                                Text("Merge with \(next.name)?")
                                    .font(.system(size: 16))
                                    .foregroundStyle(OBCTheme.ink)
                                    .lineLimit(1)
                                Spacer(minLength: 0)
                                if !expanded {
                                    Image(systemName: "chevron.right")
                                        .font(.system(size: 13, weight: .semibold))
                                        .foregroundStyle(OBCTheme.inkFaint)
                                }
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("detail.mergeSuggestion")
                        if !expanded {
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
                    }
                    if expanded {
                        Group {
                            Text(next.mergeLine)
                                .font(.obcMono(size: 13))
                                .foregroundStyle(OBCTheme.inkSoft)
                            HStack(spacing: 10) {
                                Button("Merge", action: onMerge)
                                    .buttonStyle(OBCButtonStyle(kind: .primary, fullWidth: false))
                                    .accessibilityIdentifier("detail.mergeSuggestion.merge")
                                Button("Cancel") { expanded = false }
                                    .buttonStyle(OBCButtonStyle(kind: .ghost, fullWidth: false))
                                    .accessibilityIdentifier("detail.mergeSuggestion.cancel")
                            }
                        }
                        .padding(.leading, 25)
                    }
                }
                .padding(.vertical, expanded ? 12 : 6)
                .overlay(alignment: .top) { Rectangle().fill(OBCTheme.line).frame(height: 1) }
                .overlay(alignment: .bottom) { Rectangle().fill(OBCTheme.line).frame(height: 1) }
                .animation(.default, value: expanded)
            }
        }
        .task { next = await load() }
    }
}

extension RideSummary {
    /// "Sep 30, 1:40 PM · 12.4 km": which ride a merge joins on.
    var mergeLine: String {
        "\(OBCFormat.rideDateLine(date)) · \(OBCFormat.distance(meters: distanceMeters))"
    }
}
