import SwiftUI
import OBCDomain
import OBCTransport

/// The trip page, behind a trip card in the routes list: the map in day colours, the totals, the
/// Upload trip action, one row per day with its number and its name, then the start date and the
/// bike type. A tap on a day renames it; a long press also offers
/// to end the day at a stop. A tap on a transfer line labels it. The overflow menu carries
/// Rename, Reverse and Delete trip.
///
/// Once the trip has a ride, the page is the trip review: the line with the ridden part, the
/// totals so far, one journal entry per ridden day, and the days still to ride as rows.
///
/// Driven straight off `MainScreenModel`: the model owns the trip edits and the library, and this
/// view binds them. It pops itself the moment the trip is deleted.
public struct TripDetailView: View {
    @Bindable private var model: MainScreenModel
    private let tripID: TripID
    private let onClose: () -> Void
    private let onOpenRide: (RideID) -> Void
    /// Even out the days of a re-balance offer. The offer shows only with it.
    private let onEvenOut: ((RebalanceOffer) -> Void)?
    private let onEditDays: () -> Void

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
    /// The stops sheet of one day end.
    @State private var stopsModel: TripStopsModel?
    /// The trip review, once the trip has a ride.
    @State private var journal: TripJournalModel?
    /// Each open of the page, and each return from a ride, which may bring a new note or new photos,
    /// loads the journal again.
    @State private var appearances = 0

    @Environment(\.obcIsOnline) private var isOnline

    public init(
        model: MainScreenModel,
        tripID: TripID,
        onClose: @escaping () -> Void = {},
        onOpenRide: @escaping (RideID) -> Void = { _ in },
        onEvenOut: ((RebalanceOffer) -> Void)? = nil,
        onEditDays: @escaping () -> Void = {}
    ) {
        self.model = model
        self.tripID = tripID
        self.onClose = onClose
        self.onOpenRide = onOpenRide
        self.onEvenOut = onEvenOut
        self.onEditDays = onEditDays
    }

    private var editDaysButton: some View {
        Button("Edit days", action: onEditDays)
            .buttonStyle(.obcGhost)
            .padding(.top, 12)
            .accessibilityIdentifier("trip.editDays")
    }

    private var trip: Trip? { model.trip(tripID) }
    private var days: [RouteSummary] { model.tripDays(tripID).map { $0.summary(tripID: tripID) } }

    /// What the journal is built from: a change to either builds it again.
    private struct JournalInput: Equatable {
        let trip: Trip?
        let rides: [RideSummary]
        let appearances: Int
    }

    private var journalInput: JournalInput {
        JournalInput(
            trip: trip, rides: trip.map { trip in model.rides.filter { $0.trip?.key == trip.key } } ?? [],
            appearances: appearances)
    }

    public var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                if let journal, let review = journal.review, let shown = journal.trip {
                    journalHeader(journal, review, shown)
                    journalEntries(journal, shown)
                    let unridden = review.days.indices.filter { review.days[$0].rides.isEmpty }
                    if !unridden.isEmpty {
                        OBCEyebrow("Still to ride")
                            .padding(.top, 30)
                            .padding(.bottom, 8)
                            .padding(.leading, 4)
                        dayRows(unridden)
                        editDaysButton
                    }
                    uploadButton.padding(.top, 14)
                } else {
                    header
                    dayRows(Array(days.indices))
                        .padding(.top, 14)
                    editDaysButton
                }
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
        .navigationTitle(journal?.review == nil ? trip?.name ?? "Trip" : "Trip")
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .toolbar { overflowMenu }
        .accessibilityIdentifier("trip.screen")
        .onAppear { appearances += 1 }
        // Once per open: the first pass runs before `onAppear` counts it.
        .task(id: journalInput) {
            guard appearances > 0, let trip else { return }
            let journal = journal ?? model.tripJournal()
            self.journal = journal
            await journal.load(trip: trip, rides: journalInput.rides)
        }
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
        .sheet(item: $stopsModel) { model in
            TripStopsSheet(model: model)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
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
    private var heroMap: some View {
        expandable(MultiTrackPreviewView(stages: previewStages).frame(height: 190))
    }

    @ViewBuilder
    private func expandable(_ preview: some View) -> some View {
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
    /// trip has one, and the day's stats. A transfer line sits after a day with a transfer.
    private func dayRows(_ indices: [Int]) -> some View {
        let dates = model.tripDayDates(tripID)
        return OBCGroupedSection {
            ForEach(indices, id: \.self) { index in
                let day = days[index]
                let end = trip?.dayEnds[safe: index]
                TripDayRow(
                    color: OBCTheme.stageColor(index: index),
                    number: index + 1,
                    title: end?.title ?? end?.name.map { "to \($0)" },
                    detail: ([dates[safe: index].flatMap { $0 }.map { OBCFormat.tripDay($0) }]
                        + [OBCFormat.plannedSubtitle(day), offLineNote(end)]).compactMap { $0 }
                        .joined(separator: " · "),
                    showsDivider: index != indices.last
                ) {
                    renameDay(index)
                }
                .contextMenu {
                    Button { renameDay(index) } label: { Label("Rename day", systemImage: "pencil") }
                    if index < days.count - 1 {
                        Button {
                            stopsModel = model.tripStops(tripID, day: index, isOnline: isOnline)
                        } label: { Label("End day at a stop", systemImage: "tent") }
                        .accessibilityIdentifier("trip.day.stops")
                    }
                }
                .accessibilityIdentifier("trip.day.\(index)")
                if let meters = trip?.transferMeters(after: index) {
                    TripTransferRow(kind: end?.transfer, meters: meters) {
                        model.setTripTransfer(tripID, day: index, to: $0)
                    }
                    .accessibilityIdentifier("trip.transfer.\(index)")
                }
            }
        }
    }

    private func renameDay(_ day: Int) {
        dayDraft = trip?.dayEnds[safe: day]?.title ?? ""
        dayRename = day
    }

    /// "330 m off the line" when the day ends at a stop away from the line.
    private func offLineNote(_ end: DayEnd?) -> String? {
        guard let offset = end?.stopOffset, offset > Trip.onLineMeters else { return nil }
        return OBCFormat.stopOffset(meters: offset)
    }

    // MARK: Trip review

    /// The line with the ridden part solid in the trail colour, the rest dashed, a transfer
    /// dotted with its mark, the day ends, the photos, and where the last ride stopped.
    /// `shown` is the trip the journal read; the transfer labels come from the trip as it is now.
    private func journalMap(_ journal: TripJournalModel, _ shown: Trip) -> some View {
        let trip = shown
        let stages = journal.runs.map { run in
            switch run.kind {
            case .ridden: MultiTrackPreviewView.Stage(coordinates: run.coordinates, color: OBCTheme.trackStroke)
            case .planned: MultiTrackPreviewView.Stage(coordinates: run.coordinates, color: OBCTheme.inkSoft, dash: [5, 4])
            case .transfer: MultiTrackPreviewView.Stage(coordinates: run.coordinates, color: OBCTheme.inkSoft, dash: [1.5, 4])
            }
        }
        let transfers = journal.transfers.keys.sorted().compactMap { day -> MultiTrackPreviewView.Pin? in
            guard let start = trip.dayStart(day + 1)?.coordinate else { return nil }
            let end = trip.dayEnds[day].coordinate
            return MultiTrackPreviewView.Pin(
                coordinate: Coordinate(
                    latitude: (end.latitude + start.latitude) / 2, longitude: (end.longitude + start.longitude) / 2),
                color: OBCTheme.inkSoft, systemImage: transferKind(day)?.systemImage ?? "arrow.right")
        }
        let stopped = journal.review?.days.last { $0.endedAt != nil }?.endedAt.map {
            MultiTrackPreviewView.Pin(coordinate: trip.measuredLine.coordinate(at: $0), color: OBCTheme.forest)
        }
        let pins = trip.dayEnds.dropLast().map { MultiTrackPreviewView.Pin(coordinate: $0.coordinate, color: OBCTheme.ink) }
            + journal.photoPins.map { MultiTrackPreviewView.Pin(coordinate: $0, color: OBCTheme.water) }
            + transfers + [stopped].compactMap { $0 }
        return expandable(MultiTrackPreviewView(stages: stages, pins: pins).frame(height: 230))
            .accessibilityIdentifier("trip.journal.map")
    }

    /// The label of the transfer after `day`, as the rider set it last.
    private func transferKind(_ day: Int) -> TransferKind? {
        trip?.dayEnds[safe: day]?.transfer
    }

    private func journalHeader(_ journal: TripJournalModel, _ review: TripReview, _ shown: Trip) -> some View {
        let progress = review.currentDay.map { "day \($0 + 1) of \(shown.dayCount)" }
        let dateLine = [model.tripDateLine(tripID), progress].compactMap { $0 }.joined(separator: " · ")
        return VStack(alignment: .leading, spacing: 0) {
            journalMap(journal, shown)
            Text(trip?.name ?? shown.name)
                .font(.obcSerif(size: 28))
                .foregroundStyle(OBCTheme.ink)
                .padding(.top, 16)
            Text(model.tripDateLine(tripID) == nil ? dateLine.prefix(1).uppercased() + dateLine.dropFirst() : dateLine)
                .font(.obcMono(size: 12))
                .foregroundStyle(OBCTheme.inkFaint)
                .padding(.top, 4)
            TripReviewTotals(
                ridden: review.totals, plannedMeters: review.plannedMeters, isDone: review.currentDay == nil,
                highlights: journal.highlights
            )
            .padding(.top, 12)
            .accessibilityIdentifier("trip.journal.totals")
        }
    }

    /// The ridden days in order, each with the transfer after it and the offer under the day that
    /// ended far from its plan.
    private func journalEntries(_ journal: TripJournalModel, _ trip: Trip) -> some View {
        ForEach(journal.entries) { entry in
            TripJournalDayEntry(
                number: entry.day + 1, title: trip.dayEnds[entry.day].title, header: entry.header, note: entry.note,
                photos: entry.photos, thumbnails: entry.thumbnails, rides: entry.rides, onOpenRide: onOpenRide)
                .padding(.top, 28)
                .accessibilityIdentifier("trip.journal.day.\(entry.day)")
            if let places = journal.transfers[entry.day] {
                TripJournalTransfer(kind: transferKind(entry.day), from: places.from, to: places.to) { model.setTripTransfer(tripID, day: entry.day, to: $0) }
                    .padding(.top, 22)
                    .accessibilityIdentifier("trip.journal.transfer.\(entry.day)")
            }
            if let offer = journal.offer, offer.day == entry.day, let onEvenOut, let title = journal.offerTitle {
                OBCQuietRow(
                    systemImage: "arrow.left.and.right", title: title,
                    onOpen: {
                        journal.closeOffer()
                        onEvenOut(offer)
                    },
                    onDismiss: { withAnimation(.snappy) { journal.closeOffer() } }
                )
                .padding(.top, 16)
                .accessibilityIdentifier("trip.journal.offer")
            }
        }
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

            uploadButton
        }
    }

    /// The primary action: one tap pushes the whole trip. Link-bound, so it dims when
    /// disconnected, and disabled when the trip is already fully up to date. The tap re-reads the
    /// device catalogs first, so a retry after a failed upload plans against what actually landed
    /// and never mints a duplicate from a pre-failure cache.
    private var uploadButton: some View {
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
