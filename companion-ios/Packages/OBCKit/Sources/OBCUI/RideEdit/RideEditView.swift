import SwiftUI
import OBCDomain

/// Edit mode: the ride on a tall map and its profile with the shared handles, the line that
/// says what Save keeps, and the actions Trim, Split here and Merge with next.
public struct RideEditView: View {
    /// What the rider chose. The host applies it after the screen closes.
    public enum Edit: Equatable, Sendable {
        case trim(ClosedRange<Date>)
        case split(Date)
        case mergeWithNext
    }

    @State private var model: RideEditModel
    private let name: String
    private let nextRide: RideSummary?
    private let onClose: (Edit?) -> Void
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
                        .font(.obcMono(size: 14, weight: .medium))
                        .foregroundStyle(OBCTheme.ink)
                        .padding(.bottom, 10)
                        .accessibilityIdentifier("rideEdit.summary")
                }
            }
            .background(OBCTheme.parchment.ignoresSafeArea())
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
            .rideMergeConfirmation(next: nextRide, isPresented: $mergeShown) { onClose(.mergeWithNext) }
        }
        .tint(OBCTheme.tint)
        .accessibilityIdentifier("rideEdit.screen")
    }

    private var actions: some View {
        HStack(spacing: 0) {
            action("Trim", selected: model.mode == .trim, id: "trim") { model.select(.trim) }
            action("Split here", selected: model.mode == .split, id: "split") { model.select(.split) }
            action("Merge with next", selected: false, id: "merge") { mergeShown = true }
                .disabled(nextRide == nil)
        }
        .background(OBCTheme.panel.ignoresSafeArea(edges: .bottom))
        .overlay(alignment: .top) { Rectangle().fill(OBCTheme.line).frame(height: 1) }
    }

    private func action(_ title: String, selected: Bool, id: String, perform: @escaping () -> Void) -> some View {
        Button(action: perform) {
            Text(title)
                .font(.system(size: 16, weight: selected ? .semibold : .regular))
                .foregroundStyle(selected ? OBCTheme.forest : OBCTheme.inkSoft)
                .frame(maxWidth: .infinity, minHeight: 50)
                .overlay(alignment: .bottom) {
                    if selected {
                        Capsule().fill(OBCTheme.forest).frame(width: 36, height: 3).padding(.bottom, 6)
                    }
                }
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
        .accessibilityIdentifier("rideEdit.\(id)")
    }

    private func save() {
        if let range = model.trimRange {
            onClose(.trim(range))
        } else if let time = model.splitTime {
            onClose(.split(time))
        }
    }
}

extension View {
    /// "Merge with Day 2 Ulrichen (2)?" with the next ride's start and length, and one Merge
    /// button. Shared by edit mode and the merge suggestion.
    func rideMergeConfirmation(
        next: RideSummary?, isPresented: Binding<Bool>, onMerge: @escaping () -> Void
    ) -> some View {
        confirmationDialog(
            "Merge with \(next?.name ?? "the next ride")?",
            isPresented: isPresented,
            titleVisibility: .visible
        ) {
            Button("Merge", action: onMerge)
        } message: {
            if let next {
                Text("\(OBCFormat.rideDateLine(next.date)) · \(OBCFormat.distance(meters: next.distanceMeters))")
            }
        }
    }
}
