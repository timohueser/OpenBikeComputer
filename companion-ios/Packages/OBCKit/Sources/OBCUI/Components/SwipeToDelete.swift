import SwiftUI

public extension View {
    /// Recoverable deletion supports full swipe. Irreversible deletion reveals an action that
    /// opens the caller's confirmation without removing the row.
    func obcSwipeToDelete(
        deleteTitle: String = "Delete",
        requiresConfirmation: Bool = false,
        onDelete: @escaping () -> Void
    ) -> some View {
        swipeActions(edge: .trailing, allowsFullSwipe: !requiresConfirmation) {
            Button(role: requiresConfirmation ? nil : .destructive, action: onDelete) {
                Label(deleteTitle, systemImage: "trash")
            }
            .tint(OBCTheme.danger)
        }
    }
}
