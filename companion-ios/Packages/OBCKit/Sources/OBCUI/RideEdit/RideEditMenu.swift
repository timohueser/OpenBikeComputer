import SwiftUI
import OBCDomain

/// The ride detail's ⋯ menu: Edit ride, and Revert to original on an edited ride. Edit mode
/// opens full screen; its edit applies after the screen has closed, so the detail that
/// presented it can rebuild.
public struct RideEditMenu: View {
    private let ride: Ride
    private let nextRide: RideSummary?
    private let isEdited: Bool
    private let onEdit: (RideEditView.Edit) -> Void
    private let onRevert: () -> Void
    @State private var editorShown = false
    @State private var pending: RideEditView.Edit?
    @State private var revertShown = false

    public init(
        ride: Ride,
        nextRide: RideSummary?,
        isEdited: Bool,
        onEdit: @escaping (RideEditView.Edit) -> Void,
        onRevert: @escaping () -> Void
    ) {
        self.ride = ride
        self.nextRide = nextRide
        self.isEdited = isEdited
        self.onEdit = onEdit
        self.onRevert = onRevert
    }

    public var body: some View {
        Menu {
            Button { editorShown = true } label: { Label("Edit ride", systemImage: "scissors") }
                .accessibilityIdentifier("detail.editRide")
            if isEdited {
                Button { revertShown = true } label: {
                    Label("Revert to original", systemImage: "arrow.uturn.backward")
                }
                .accessibilityIdentifier("detail.revertRide")
            }
        } label: {
            Image(systemName: "ellipsis.circle")
        }
        .accessibilityLabel("More")
        .accessibilityIdentifier("detail.overflow")
        .confirmationDialog("Revert to original?", isPresented: $revertShown, titleVisibility: .visible) {
            Button("Revert", role: .destructive, action: onRevert)
        } message: {
            Text("The ride shows as it was synced.")
        }
        #if os(iOS)
        .fullScreenCover(isPresented: $editorShown, onDismiss: applyPending) { editorScreen }
        #else
        .sheet(isPresented: $editorShown, onDismiss: applyPending) { editorScreen }
        #endif
    }

    @ViewBuilder
    private var editorScreen: some View {
        RideEditView(ride: ride, nextRide: nextRide) { edit in
            pending = edit
            editorShown = false
        }
    }

    private func applyPending() {
        guard let edit = pending else { return }
        pending = nil
        onEdit(edit)
    }
}
