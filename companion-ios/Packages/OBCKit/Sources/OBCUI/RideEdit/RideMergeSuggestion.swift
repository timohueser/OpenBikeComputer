import SwiftUI
import OBCDomain

/// The quiet row over a ride's ledger: "Merge with Day 2 Ulrichen (2)?". A tap opens the
/// row in place with the next ride's start and length, and Merge and Cancel; ✕ or a swipe
/// dismisses it for good. `load` finds the next ride once, off the
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
                Group {
                    if expanded {
                        confirmation(next)
                    } else {
                        OBCQuietRow(
                            systemImage: "arrow.triangle.merge",
                            title: "Merge with \(next.name)?",
                            onOpen: { withAnimation(.snappy) { expanded = true } },
                            onDismiss: {
                                withAnimation(.snappy) { self.next = nil }
                                onDismiss()
                            }
                        )
                    }
                }
                .padding(.top, 14)
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("detail.mergeSuggestion")
            }
        }
        .task { next = await load() }
    }

    /// The row opened in place: the next ride's start and length, and Merge and Cancel.
    private func confirmation(_ next: RideSummary) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Image(systemName: "arrow.triangle.merge")
                    .font(.system(.subheadline, weight: .medium))
                    .foregroundStyle(OBCTheme.secondary)
                Text("Merge with \(next.name)?")
                    .font(.system(.subheadline))
                    .foregroundStyle(OBCTheme.ink)
            }
            Text(next.mergeLine)
                .font(.system(.footnote).monospacedDigit())
                .foregroundStyle(OBCTheme.secondary)
            HStack(spacing: 10) {
                Button("Merge", action: onMerge)
                    .buttonStyle(OBCButtonStyle(kind: .primary, fullWidth: false))
                    .accessibilityIdentifier("detail.mergeSuggestion.merge")
                Button("Cancel") { withAnimation(.snappy) { expanded = false } }
                    .buttonStyle(OBCButtonStyle(kind: .ghost, fullWidth: false))
                    .accessibilityIdentifier("detail.mergeSuggestion.cancel")
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
    }
}

extension RideSummary {
    /// "Sep 30, 1:40 PM · 12.4 km": which ride a merge joins on.
    var mergeLine: String {
        "\(OBCFormat.rideDateLine(date)) · \(OBCFormat.distance(meters: distanceMeters))"
    }
}
