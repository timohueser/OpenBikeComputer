import SwiftUI

public extension View {
    /// Swipe-to-delete row — trailing-swipe reveal exposing a warning-red
    /// Delete that removes the row directly (the swipe reveal is already the
    /// second, deliberate action — no extra confirm); full-swipe destroys
    /// too, the standard iOS gesture. Apply to a row inside a `List`:
    ///
    ///     List(routes) { route in
    ///         RouteCard(route: route)
    ///             .obcSwipeToDelete { … }
    ///     }
    ///
    /// One-tap destructive entry points (a detail screen's Delete button) still
    /// confirm via `.obcDestructiveConfirm`.
    func obcSwipeToDelete(
        deleteTitle: String = "Delete",
        onDelete: @escaping () -> Void
    ) -> some View {
        swipeActions(edge: .trailing, allowsFullSwipe: true) {
            Button(role: .destructive, action: onDelete) {
                Label(deleteTitle, systemImage: "trash")
            }
            .tint(OBCTheme.warning)
        }
    }
}
