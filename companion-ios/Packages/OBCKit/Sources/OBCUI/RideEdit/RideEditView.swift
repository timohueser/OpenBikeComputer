import SwiftUI
import OBCDomain

/// Edit mode: the ride on a tall map and its profile with the two trim handles, the line that
/// says what Save keeps, and Merge with next.
public struct RideEditView: View {
    /// What the rider chose. The host applies it after the screen closes.
    public enum Edit: Equatable, Sendable {
        case trim(ClosedRange<Date>)
        case mergeWithNext
    }

    @State private var model: RideEditModel
    private let name: String
    private let nextRide: RideSummary?
    private let onClose: (Edit?) -> Void
    /// Merge with next expands the toolbar in place with the next ride and Merge and Cancel.
    @State private var mergeShown = false

    /// Nil for a ride with fewer than two points.
    public init?(ride: Ride, nextRide: RideSummary?, onClose: @escaping (Edit?) -> Void) {
        guard let model = RideEditModel(ride: ride) else { return nil }
        _model = State(initialValue: model)
        name = ride.summary.name
        self.nextRide = nextRide
        self.onClose = onClose
    }

    public var body: some View {
        NavigationStack {
            GeometryReader { geometry in
                VStack(spacing: 0) {
                    // The profile and the lines below take about 230 pt; the map gets the rest.
                    LineMarkerEditor(model: model.editor, mapHeight: max(200, geometry.size.height - 230))
                        .padding(.horizontal, 8)
                        .padding(.top, 8)
                    Spacer(minLength: 0)
                    Text(model.summaryLine)
                        .font(.system(.subheadline, weight: .medium).monospacedDigit())
                        .foregroundStyle(OBCTheme.ink)
                        .padding(.bottom, 10)
                        .accessibilityIdentifier("rideEdit.summary")
                }
            }
            .background(OBCTheme.page.ignoresSafeArea())
            .safeAreaInset(edge: .bottom, spacing: 0) { actions }
            .navigationTitle(name)
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { onClose(nil) }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save", action: save)
                        .fontWeight(.semibold)
                        .disabled(!model.canSave)
                        .accessibilityIdentifier("rideEdit.save")
                }
            }
        }
        .tint(OBCTheme.tint)
        .accessibilityIdentifier("rideEdit.screen")
    }

    @ViewBuilder
    private var actions: some View {
        Group {
            if mergeShown, let nextRide {
                VStack(alignment: .leading, spacing: 10) {
                    Text("Merge with \(nextRide.name)?")
                        .font(.system(.callout, weight: .semibold))
                        .foregroundStyle(OBCTheme.ink)
                    Text(nextRide.mergeLine)
                        .font(.system(.footnote).monospacedDigit())
                        .foregroundStyle(OBCTheme.secondary)
                    HStack(spacing: 10) {
                        Button("Merge") { onClose(.mergeWithNext) }
                            .buttonStyle(.obcPrimary)
                            .accessibilityIdentifier("rideEdit.mergeConfirm")
                        Button("Cancel") { mergeShown = false }
                            .buttonStyle(.obcGhost)
                            .accessibilityIdentifier("rideEdit.mergeCancel")
                    }
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 16)
                .frame(maxWidth: .infinity, alignment: .leading)
            } else {
                Button { mergeShown = true } label: {
                    Label("Merge with next ride", systemImage: "arrow.triangle.merge")
                }
                .buttonStyle(.obcGhost)
                .disabled(nextRide == nil)
                .padding(.horizontal, 20)
                .padding(.vertical, 12)
                .accessibilityIdentifier("rideEdit.merge")
            }
        }
        .background(OBCTheme.surface.ignoresSafeArea(edges: .bottom))
        .overlay(alignment: .top) { Rectangle().fill(OBCTheme.hairline).frame(height: 1) }
        .animation(.default, value: mergeShown)
    }

    private func save() {
        if let range = model.trimRange { onClose(.trim(range)) }
    }
}
