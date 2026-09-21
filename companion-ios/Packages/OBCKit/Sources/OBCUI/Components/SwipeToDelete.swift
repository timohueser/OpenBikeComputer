import SwiftUI

public extension View {
    /// A trailing-swipe reveal exposing a warning-red Delete that removes the row
    /// directly: the swipe reveal is already the second, deliberate action, so there
    /// is no extra confirm. A full swipe destroys too, as the standard iOS gesture.
    /// Apply it to a row inside a `List`. One-tap destructive entry points, such as a
    /// detail screen's Delete button, still confirm through `.obcDestructiveConfirm`.
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
