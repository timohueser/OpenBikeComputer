import SwiftUI
import OBCDomain

/// The choice the shared trip picker returns: leave the route loose, file it in an
/// existing trip, or start a new trip. Every filing flow resolves to one of these.
public enum TripSelection: Equatable, Sendable {
    /// Do not file the route.
    case none
    /// Add the route to an existing trip as its new last day.
    case existing(TripID)
    /// Start a new trip with this trimmed, non-empty name, with the route as its first day.
    case new(String)
}

/// A light projection of a `Trip` for the picker's rows, so the picker never
/// depends on the whole library type.
public struct TripPickerItem: Identifiable, Equatable, Sendable {
    public let id: TripID
    public let name: String
    public let dayCount: Int

    public init(id: TripID, name: String, dayCount: Int) {
        self.id = id
        self.name = name
        self.dayCount = dayCount
    }
}

/// The one trip picker: a sheet offering the existing trips plus a New trip inline
/// name field. `allowsNone` adds the opt-in "Don't add to a trip" row. Picking a row
/// calls `onPick` once and dismisses; Cancel dismisses with nothing. Filing is the
/// caller's job, so the same sheet drives an import save and the route menus alike.
public struct TripPickerSheet: View {
    private let title: String
    private let trips: [TripPickerItem]
    private let allowsNone: Bool
    private let currentTripID: TripID?
    private let onPick: (TripSelection) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var creatingNew = false
    @State private var newName = "New trip"
    @FocusState private var nameFocused: Bool

    public init(
        title: String,
        trips: [TripPickerItem],
        allowsNone: Bool = false,
        currentTripID: TripID? = nil,
        onPick: @escaping (TripSelection) -> Void
    ) {
        self.title = title
        self.trips = trips
        self.allowsNone = allowsNone
        self.currentTripID = currentTripID
        self.onPick = onPick
    }

    public var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 20) {
                    OBCGroupedSection { newTripArea }

                    if !trips.isEmpty || allowsNone {
                        OBCGroupedSection(trips.isEmpty ? nil : "Existing trips") {
                            ForEach(trips) { item in
                                OBCListRow(
                                    icon: "folder",
                                    iconColor: OBCTheme.wood,
                                    label: item.name,
                                    value: "\(item.dayCount) \(item.dayCount == 1 ? "day" : "days")",
                                    showsDivider: item.id != trips.last?.id || allowsNone,
                                    action: { pick(.existing(item.id)) },
                                    trailing: {
                                        if item.id == currentTripID {
                                            Image(systemName: "checkmark")
                                                .font(.system(size: 14, weight: .semibold))
                                                .foregroundStyle(OBCTheme.forest)
                                        }
                                    }
                                )
                                .accessibilityIdentifier("tripPicker.trip.\(item.id.rawValue)")
                            }
                            if allowsNone {
                                OBCListRow(
                                    icon: "minus.circle",
                                    iconColor: OBCTheme.inkSoft,
                                    label: "Don't add to a trip",
                                    showsDivider: false,
                                    action: { pick(.none) }
                                )
                                .accessibilityIdentifier("tripPicker.none")
                            }
                        }
                    }
                }
                .padding(20)
            }
            .background(OBCTheme.parchment.ignoresSafeArea())
            .navigationTitle(title)
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
            .accessibilityIdentifier("tripPicker.screen")
        }
        .tint(OBCTheme.tint)
    }

    /// A plain row that expands into an inline name field and a Create action.
    @ViewBuilder
    private var newTripArea: some View {
        if creatingNew {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 12) {
                    OBCIconTile(systemImage: "folder.badge.plus", color: OBCTheme.forest)
                    TextField("Trip name", text: $newName)
                        .font(.system(size: 16))
                        .focused($nameFocused)
                        .submitLabel(.done)
                        .onSubmit { create() }
                        .accessibilityIdentifier("tripPicker.newName")
                }
                Button("Create trip") { create() }
                    .buttonStyle(.obcPrimary)
                    .disabled(trimmedName.isEmpty)
                    .accessibilityIdentifier("tripPicker.create")
            }
            .padding(16)
        } else {
            OBCListRow(
                icon: "folder.badge.plus",
                iconColor: OBCTheme.forest,
                label: "New trip…",
                showsChevron: true,
                showsDivider: false,
                action: {
                    creatingNew = true
                    nameFocused = true
                }
            )
            .accessibilityIdentifier("tripPicker.newTrip")
        }
    }

    private var trimmedName: String {
        newName.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func create() {
        guard !trimmedName.isEmpty else { return }
        pick(.new(trimmedName))
    }

    private func pick(_ selection: TripSelection) {
        onPick(selection)
        dismiss()
    }
}

#if DEBUG
#Preview("Trip picker") {
    Color.clear.sheet(isPresented: .constant(true)) {
        TripPickerSheet(
            title: "Add to trip",
            trips: [
                TripPickerItem(id: TripID("a"), name: "Driftless Weekender", dayCount: 2),
                TripPickerItem(id: TripID("b"), name: "Alpine Traverse", dayCount: 5),
            ],
            allowsNone: true,
            onPick: { _ in }
        )
    }
}
#endif
