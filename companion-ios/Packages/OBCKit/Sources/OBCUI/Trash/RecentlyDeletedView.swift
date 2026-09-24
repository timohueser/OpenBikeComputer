import SwiftUI
import OBCDomain

/// The phone-side ride trash, pushed from the row under the Tracked list. Deleting a
/// ride in the app is recoverable: the stored files stay until this screen removes
/// them for good, or the retention sweep does.
///
/// Rides only. A planned route stays a hard delete because it is a re-importable
/// file, while a ride is the only copy of a recording the phone has. The device's
/// SD-card copy is never touched from here either way.
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
                    Text(
                        "Rides stay here for \(MainScreenModel.trashRetentionDays) days, "
                            + "then they're removed for good. The copies on your OBC aren't touched."
                    )
                    .font(.system(.footnote))
                    .foregroundStyle(OBCTheme.secondary)
                    .lineSpacing(3)
                    .listRowSeparator(.hidden)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 12, trailing: 20))

                    let rides = model.trashedRides
                    ForEach(Array(rides.enumerated()), id: \.element.id) { index, ride in
                        Button {
                            selected = ride
                        } label: {
                            TrackRow(ride: ride)
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("trash.card.\(ride.id.rawValue)")
                        .swipeActions(edge: .leading) {
                            Button {
                                model.recoverRide(ride.id)
                            } label: {
                                Label("Recover", systemImage: "arrow.uturn.backward")
                            }
                            .tint(OBCTheme.tint)
                        }
                        .obcSwipeToDelete {
                            model.deleteRideForever(ride.id)
                        }
                        .obcGroupedRow(first: index == 0, last: index == rides.count - 1)
                    }
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
            }
        }
        .background(OBCTheme.page.ignoresSafeArea())
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
