import SwiftUI
import OBCDomain
import OBCTransport

/// The trip page, behind a trip card in the routes list: the map in day colours, the totals, the
/// Upload trip action, one row per day with its number and its name, then the start date and the
/// bike type. A tap on a day renames it. The overflow menu carries Rename, Reverse and Delete trip.
///
/// Driven straight off `MainScreenModel`: the model owns the trip edits and the library, and this
/// view binds them. It pops itself the moment the trip is deleted.
public struct TripDetailView: View {
    @Bindable private var model: MainScreenModel
    private let tripID: TripID
    private let onClose: () -> Void

    @State private var renameShown = false
    @State private var renameDraft = ""
    @State private var deleteDialogShown = false
    /// The whole-trip upload sheet's driver, created once at the Upload tap. A model built inline
    /// in the `.sheet` closure would rebuild on every body pass and restart the queue.
    @State private var tripUploadModel: TripUploadModel?
    /// Upload tapped with the catalog re-read in flight: it debounces the button until the sheet's
    /// driver exists.
    @State private var isPreparingUpload = false
    @State private var dayRename: Int?
    @State private var dayDraft = ""
    @State private var startDateShown = false
    /// The full-screen interactive trip map.
    @State private var mapShown = false

    @Environment(\.obcIsOnline) private var isOnline

    public init(
        model: MainScreenModel,
        tripID: TripID,
        onClose: @escaping () -> Void = {}
    ) {
        self.model = model
        self.tripID = tripID
        self.onClose = onClose
    }

    private var trip: Trip? { model.trip(tripID) }
    private var days: [RouteSummary] { model.tripDays(tripID).map { $0.summary(tripID: tripID) } }

    public var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                dayRows
                    .padding(.top, 14)
                OBCGroupedSection {
                    OBCListRow(
                        label: "Start date",
                        value: trip?.startDay.map { OBCFormat.tripDay($0) } ?? "None",
                        showsChevron: true
                    ) { startDateShown = true }
                    .accessibilityIdentifier("trip.startDate")
                    OBCBikeTypeRow(type: trip?.bikeType ?? .road) { model.setTripBikeType(tripID, to: $0) }
                        .accessibilityIdentifier("trip.bikeType")
                }
                .padding(.top, 14)
            }
            .padding(.horizontal, 20)
            .padding(.top, 4)
            .padding(.bottom, 24)
        }
        .background(OBCTheme.parchment.ignoresSafeArea())
        .navigationTitle(trip?.name ?? "Trip")
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .toolbar { overflowMenu }
        .accessibilityIdentifier("trip.screen")
        .obcRenameAlert(
            "Rename trip",
            isPresented: $renameShown,
            name: $renameDraft,
            onSave: {
                let name = renameDraft.trimmingCharacters(in: .whitespacesAndNewlines)
                if !name.isEmpty { model.renameTrip(tripID, to: name) }
            }
        )
        .confirmationDialog(
            "Delete \(trip?.name.quoted ?? "trip")?",
            isPresented: $deleteDialogShown,
            titleVisibility: .visible
        ) {
            Button("Delete trip", role: .destructive) {
                model.deleteTrip(tripID)
                onClose()
            }
            Button("Cancel", role: .cancel) {}
        }
        .obcRenameAlert(
            "Rename day",
            isPresented: Binding(get: { dayRename != nil }, set: { if !$0 { dayRename = nil } }),
            name: $dayDraft,
            onSave: {
                if let day = dayRename { model.renameTripDay(tripID, day: day, to: dayDraft) }
            }
        )
        .sheet(isPresented: $startDateShown) {
            TripStartDateSheet(startDay: trip?.startDay) { model.setTripStartDay(tripID, to: $0) }
        }
        // The trip vanished under us, so leave the page.
        .onChange(of: model.trips) { _, _ in
            if model.trip(tripID) == nil { onClose() }
        }
        .sheet(item: $tripUploadModel) { model in
            TripUploadSheetView(model: model)
        }
        #if os(iOS)
        .fullScreenCover(isPresented: $mapShown) { tripMapCover }
        #else
        .sheet(isPresented: $mapShown) { tripMapCover }
        #endif
    }

    // MARK: Header

    /// The days as coloured preview tracks: the shared input of the hero map and the full-screen
    /// map, with the palette colour by day index.
    private var previewStages: [MultiTrackPreviewView.Stage] {
        days.enumerated().map { index, summary in
            MultiTrackPreviewView.Stage(
                coordinates: summary.trackPreview?.coordinates ?? [],
                color: OBCTheme.stageColor(index: index)
            )
        }
    }

    /// The hero can expand to the interactive map when there is real geometry and a network path,
    /// the route detail's rule verbatim.
    private var canExpandMap: Bool {
        isOnline && previewStages.contains { !$0.coordinates.isEmpty }
    }

    /// The whole-trip hero map: every day in its palette colour, above the stat strip. Tapping
    /// it, online and with geometry, opens the full-screen interactive map, the same affordance as
    /// the route detail's hero.
    @ViewBuilder
    private var heroMap: some View {
        let preview = MultiTrackPreviewView(stages: previewStages)
            .frame(height: 190)

        if canExpandMap {
            Button { mapShown = true } label: {
                // The preview ignores hits, because the tap is ours, so make the whole hero the
                // tap target.
                preview.contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("trip.expandMap")
            .accessibilityLabel("Open full trip map")
        } else {
            preview
        }
    }

    private var tripMapCover: some View {
        TrackMapView(
            stages: model.tripDays(tripID).enumerated().map { index, day in
                MultiTrackPreviewView.Stage(
                    coordinates: day.points.map(\.coordinate), color: OBCTheme.stageColor(index: index))
            },
            stageSummaries: days,
            title: trip?.name ?? "Trip",
            onClose: { mapShown = false }
        )
    }

    /// One row per day: the number, the day's own name or "to ‹place›", then the date, when the
    /// trip has one, and the day's stats. A transfer line sits between two days with a transfer.
    private var dayRows: some View {
        let routes = model.tripDays(tripID)
        let dates = model.tripDayDates(tripID)
        let transfers = Trip.transferMeters(between: routes.map(\.points))
        return OBCGroupedSection {
            ForEach(Array(days.enumerated()), id: \.element.id) { index, day in
                let end = trip?.dayEnds[safe: index]
                TripDayRow(
                    color: OBCTheme.stageColor(index: index),
                    number: index + 1,
                    title: end?.title ?? end?.name.map { "to \($0)" },
                    detail: ([dates[safe: index].flatMap { $0 }.map { OBCFormat.tripDay($0) }]
                        + [OBCFormat.plannedSubtitle(day)]).compactMap { $0 }.joined(separator: " · "),
                    showsDivider: index < routes.count - 1
                ) {
                    dayDraft = end?.title ?? ""
                    dayRename = index
                }
                .accessibilityIdentifier("trip.day.\(index)")
                if let meters = transfers[safe: index] ?? nil {
                    transferLine(meters: meters)
                        .accessibilityIdentifier("trip.transfer.\(index)")
                }
            }
        }
    }

    /// "Transfer · 80 km", in the type of the join sheet's "joins" and "gap" lines.
    private func transferLine(meters: Double) -> some View {
        Text("Transfer · \(OBCFormat.distance(meters: meters))")
            .font(.obcMono(size: 12))
            .foregroundStyle(OBCTheme.inkFaint)
            .padding(.vertical, 8)
            .padding(.leading, 38)
            .frame(maxWidth: .infinity, alignment: .leading)
            .overlay(alignment: .bottom) { OBCTheme.screenLine.frame(height: 1).padding(.leading, 38) }
    }

    private var header: some View {
        let stats = model.tripStats(tripID)
        return VStack(alignment: .leading, spacing: 14) {
            heroMap

            OBCStatStrip([
                OBCStat(
                    value: OBCFormat.distanceValue(meters: stats.distanceMeters), unit: "km",
                    key: "Distance"),
                OBCStat(
                    value: OBCFormat.climbValue(meters: stats.elevationGainMeters), unit: "m",
                    key: "Climb"),
                OBCStat(value: "\(stats.dayCount)", key: stats.dayCount == 1 ? "Day" : "Days"),
            ])
            .accessibilityIdentifier("trip.stats")

            // The primary action: one tap pushes the whole trip. Link-bound, so it dims when
            // disconnected, and disabled when the trip is already fully up to date. The tap
            // re-reads the device catalogs first, so a retry after a failed upload plans against
            // what actually landed and never mints a duplicate from a pre-failure cache.
            Button {
                guard !isPreparingUpload else { return }
                isPreparingUpload = true
                Task {
                    tripUploadModel = await model.prepareTripUpload(tripID)
                    isPreparingUpload = false
                }
            } label: {
                Label("Upload trip", systemImage: "square.and.arrow.up")
            }
            .buttonStyle(.obcPrimary)
            .disabled(!canUploadTrip || isPreparingUpload)
            .accessibilityIdentifier("trip.upload")
        }
    }

    /// Upload is offered while connected and the trip is not already fully current on the device.
    private var canUploadTrip: Bool {
        model.connection == .connected && model.tripOnDeviceState(tripID) != .upToDate
    }

    // MARK: Overflow

    private var overflowMenu: some ToolbarContent {
        ToolbarItem(placement: .primaryAction) {
            Menu {
                Button {
                    renameDraft = trip?.name ?? ""
                    renameShown = true
                } label: { Label("Rename", systemImage: "pencil") }
                .accessibilityIdentifier("trip.rename")

                Button { model.reverseTrip(tripID) } label: {
                    Label("Reverse", systemImage: "arrow.left.arrow.right")
                }
                .accessibilityIdentifier("trip.reverse")

                Divider()

                Button(role: .destructive) { deleteDialogShown = true } label: {
                    Label("Delete trip…", systemImage: "trash")
                }
                .accessibilityIdentifier("trip.delete")
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .accessibilityIdentifier("trip.overflow")
        }
    }
}

extension String {
    /// The string wrapped in typographic double quotes, the dialog-title idiom.
    fileprivate var quoted: String { "\u{201C}\(self)\u{201D}" }
}

extension Array {
    fileprivate subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}
