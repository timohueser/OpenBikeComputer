import SwiftUI
import OBCDomain

/// The choice returned by the **shared trip picker** (TR7): leave the route
/// loose, file it in an existing trip, or start a new trip under a name. The
/// one selection type every filing flow speaks — multi-select grouping, the
/// import row, and the route menus all resolve to one of these three.
public enum TripSelection: Equatable, Sendable {
    /// Don't file the route (the import row's opt-in default; never offered by
    /// the route menus, where Remove is a separate action).
    case none
    /// File into an existing trip — the ≤ 1-trip invariant makes this an
    /// implicit move when the route already sits in another trip.
    case existing(TripID)
    /// Start a new trip with this (trimmed, non-empty) name and file the route
    /// as its first stage.
    case new(String)
}

/// A light projection of a `TripRecord` for the picker's rows — name + stage
/// count, keyed by ``TripID`` — so the picker never depends on the whole
/// library type. Built once by `MainScreenModel.tripPickerItems`.
public struct TripPickerItem: Identifiable, Equatable, Sendable {
    public let id: TripID
    public let name: String
    public let stageCount: Int

    public init(id: TripID, name: String, stageCount: Int) {
        self.id = id
        self.name = name
        self.stageCount = stageCount
    }
}

/// **The** trip picker (TR7, locked: all pickers are one component). A sheet
/// offering the existing trips plus a **New trip…** inline name field; the
/// import row adds the opt-in *Don't add to a trip* row (`allowsNone`), while
/// the route menus present it without that row. Picking a row (or creating a
/// new trip) calls `onPick` once and dismisses; Cancel dismisses with nothing.
///
/// Filing itself is the caller's — the picker only reports the choice, so the
/// same sheet drives an import save, a loose route's context menu, and a filed
/// route's Move/Add overflow without knowing which.
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
                                    value: "\(item.stageCount) \(item.stageCount == 1 ? "stage" : "stages")",
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

    /// The "New trip…" affordance — a plain row that expands into an inline name
    /// field (prefilled "New trip", nothing fancier — the locked default) plus a
    /// Create action.
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
                TripPickerItem(id: TripID("a"), name: "Driftless Weekender", stageCount: 2),
                TripPickerItem(id: TripID("b"), name: "Alpine Traverse", stageCount: 5),
            ],
            allowsNone: true,
            onPick: { _ in }
        )
    }
}
#endif
