import SwiftUI
import OBCDomain

/// Recently Deleted (#292): the phone-side ride trash, pushed from the row
/// under the Tracked list. Deleting a ride in the app is recoverable — the
/// stored files stay until this screen removes them for good (or the
/// `MainScreenModel` retention sweep does, after `trashRetentionDays`).
///
/// Rides only — planned routes stay hard-delete: a route is a re-importable
/// file, a ride is the only copy of a recording the phone has. The device's
/// SD-card copy is never touched from here either way.
///
/// Row actions: tap → Recover / Delete Permanently dialog; the same two as
/// swipes (leading Recover, trailing Delete — the swipe reveal is the
/// deliberate second action, §9's rule, so the trailing delete is direct).
public struct RecentlyDeletedView: View {
    private var model: MainScreenModel

    @State private var selected: RideSummary?

    public init(model: MainScreenModel) {
        self.model = model
    }

    public var body: some View {
        Group {
            if model.trashedRides.isEmpty {
                OBCEmptyStateView(
                    glyph: .muted(systemImage: "trash"),
                    title: "Nothing here",
                    message: "Deleted rides stay here for \(MainScreenModel.trashRetentionDays) days."
                )
                .frame(maxHeight: .infinity, alignment: .top)
                .padding(.top, 60)
            } else {
                List {
                    Group {
                        Text(
                            "Rides stay here for \(MainScreenModel.trashRetentionDays) days, "
                                + "then they're removed for good. The copies on your OBC aren't touched."
                        )
                        .font(.system(size: 13))
                        .foregroundStyle(OBCTheme.inkSoft)
                        .lineSpacing(3)
                        .padding(.bottom, 4)

                        ForEach(model.trashedRides) { ride in
                            Button {
                                selected = ride
                            } label: {
                                RouteCard(ride: ride)
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("trash.card.\(ride.id.rawValue)")
                            .swipeActions(edge: .leading) {
                                Button {
                                    model.recoverRide(ride.id)
                                } label: {
                                    Label("Recover", systemImage: "arrow.uturn.backward")
                                }
                                .tint(OBCTheme.forest)
                            }
                            .obcSwipeToDelete {
                                model.deleteRideForever(ride.id)
                            }
                        }
                    }
                    .listRowSeparator(.hidden)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 12, trailing: 20))
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
            }
        }
        .background(OBCTheme.parchment.ignoresSafeArea())
        .navigationTitle("Recently Deleted")
        #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
        #endif
        .confirmationDialog(
            selected?.name ?? "",
            isPresented: Binding(
                get: { selected != nil },
                set: { if !$0 { selected = nil } }
            ),
            titleVisibility: .visible,
            presenting: selected
        ) { ride in
            Button("Recover") { model.recoverRide(ride.id) }
            Button("Delete Permanently", role: .destructive) { model.deleteRideForever(ride.id) }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("Deleting removes it from this phone for good.")
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("trash.screen")
    }
}
