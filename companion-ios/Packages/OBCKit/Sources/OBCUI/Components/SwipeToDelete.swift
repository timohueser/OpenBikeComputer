import SwiftUI

public extension View {
    /// **Swipe-to-Delete Row** (§9, NEW) — trailing-swipe reveal exposing a
    /// warning-red Delete that **always routes through the confirm sheet**
    /// (H11 → H1); full-swipe destroy is disabled on purpose. Apply to a row
    /// inside a `List`:
    ///
    ///     List(routes) { route in
    ///         RouteCard(route: route)
    ///             .obcSwipeToDelete(
    ///                 confirmTitle: "Delete \"\(route.name)\"?",
    ///                 message: "This removes it from your phone…",
    ///                 onDelete: { … }
    ///             )
    ///     }
    /// `deleteTitle` labels the revealed swipe action ("Delete"); `actionTitle`
    /// labels the confirm sheet's destructive button ("Delete route" per H1 —
    /// defaults to `deleteTitle`).
    func obcSwipeToDelete(
        confirmTitle: String,
        message: String,
        deleteTitle: String = "Delete",
        actionTitle: String? = nil,
        onDelete: @escaping () -> Void
    ) -> some View {
        modifier(OBCSwipeToDelete(
            confirmTitle: confirmTitle,
            message: message,
            deleteTitle: deleteTitle,
            actionTitle: actionTitle ?? deleteTitle,
            onDelete: onDelete
        ))
    }
}

private struct OBCSwipeToDelete: ViewModifier {
    let confirmTitle: String
    let message: String
    let deleteTitle: String
    let actionTitle: String
    let onDelete: () -> Void
    @State private var confirmShown = false

    func body(content: Content) -> some View {
        content
            .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                // NOT role .destructive: the row must survive the swipe so the
                // confirm sheet owns the actual removal.
                Button {
                    confirmShown = true
                } label: {
                    Label(deleteTitle, systemImage: "trash")
                }
                .tint(OBCTheme.warning)
            }
            .obcDestructiveConfirm(
                confirmTitle,
                isPresented: $confirmShown,
                message: message,
                actionTitle: actionTitle,
                onConfirm: onDelete
            )
    }
}
