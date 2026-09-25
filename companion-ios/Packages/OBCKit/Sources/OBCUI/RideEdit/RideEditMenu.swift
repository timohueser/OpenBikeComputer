import SwiftUI
import OBCDomain

/// The ride detail's More menu: edit, save as route, revert, and delete. Edit mode
/// opens full screen; its edit applies after the screen has closed, so the detail that
/// presented it can rebuild.
public struct RideEditMenu: View {
    private let ride: Ride
    private let nextRide: RideSummary?
    private let isEdited: Bool
    private let onEdit: (RideEditView.Edit) -> Void
    private let onRevert: () -> Void
    private let onSaveAsRoute: (() -> Void)?
    private var onDelete: (() -> Void)?
    @State private var deleteShown = false
    @State private var editorShown = false
    @State private var pending: RideEditView.Edit?
    @State private var revertShown = false

    public init(
        ride: Ride,
        nextRide: RideSummary?,
        isEdited: Bool,
        onEdit: @escaping (RideEditView.Edit) -> Void,
        onRevert: @escaping () -> Void,
        onSaveAsRoute: (() -> Void)? = nil
    ) {
        self.ride = ride
        self.nextRide = nextRide
        self.isEdited = isEdited
        self.onEdit = onEdit
        self.onRevert = onRevert
        self.onSaveAsRoute = onSaveAsRoute
    }

    public func deleteAction(_ action: (() -> Void)?) -> Self {
        var menu = self
        menu.onDelete = action
        return menu
    }

    public var body: some View {
        Menu {
            // A ride of one point has nothing to edit.
            if ride.points.count > 1 {
                Button { editorShown = true } label: { Label("Edit ride", systemImage: "scissors") }
                    .accessibilityIdentifier("detail.editRide")
            }
            Button { onSaveAsRoute?() } label: {
                Label("Save as route", systemImage: "point.topleft.down.to.point.bottomright.curvepath")
                if onSaveAsRoute == nil { Text("A gap in the ride is too long to join") }
            }
            .disabled(onSaveAsRoute == nil)
            .accessibilityIdentifier("detail.saveAsRoute")
            if isEdited {
                Button { revertShown = true } label: {
                    Label("Revert to original", systemImage: "arrow.uturn.backward")
                }
                .accessibilityIdentifier("detail.revertRide")
            }
            if onDelete != nil {
                Divider()
                Button(role: .destructive) { deleteShown = true } label: {
                    Label("Delete ride…", systemImage: "trash")
                }
                .accessibilityIdentifier("detail.delete")
            }
        } label: {
            Image(systemName: "ellipsis")
        }
        .accessibilityLabel("More")
        .accessibilityIdentifier("detail.overflow")
        .obcDestructiveConfirm(
            "Delete \"\(ride.summary.name)\"?",
            isPresented: $deleteShown,
            message: "Moves it to Recently Deleted. The ride stays on the device.",
            actionTitle: "Delete ride",
            onConfirm: { onDelete?() }
        )
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
