import SwiftUI
import OBCDomain
import OBCTransport

/// The trip page, behind a trip card in the routes list, in the route page's order: the map in
/// day colours, the name, what the device holds with the one action, the ledger, one row per day
/// with a bar as long as the day, then the start date and the bike type. A tap on a day opens its
/// route detail; its More menu offers Rename day and End day at a stop. A tap on a transfer line
/// picks how the rider travels it. The overflow menu carries Reverse and Delete trip.
///
/// Once the trip has a ride, the page is the trip review: the line with the ridden part, the
/// totals so far, one journal entry per ridden day, and the days still to ride as rows. Its share
/// button gives the planned line as GPX and the share image.
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
    private let onOpenDay: (Int) -> Void
    /// The planned line as GPX. The review's share button shows only with it.
    private let encodeGPX: (@Sendable (Trip) -> Data)?
    private let uploadTiming: TripUploadModel.Timing

    @State private var renameShown = false
    @State private var deleteDialogShown = false
    @State private var reverseDialogShown = false
    /// The whole-trip upload sheet's driver, created once at the Upload tap. A model built inline
    /// in the `.sheet` closure would rebuild on every body pass and restart the queue.
    @State private var tripUploadModel: TripUploadModel?
    /// Upload tapped with the catalog re-read in flight: it debounces the button until the sheet's
    /// driver exists.
    @State private var isPreparingUpload = false
    @State private var dayRename: Int?
    @State private var startDateShown = false
    /// The full-screen interactive trip map.
    @State private var mapShown = false
    @State private var replayShown = false
    @State private var replayPreparing = false
    @State private var replayContent: ReplayContent?
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
        onEditDays: @escaping () -> Void = {},
        onOpenDay: @escaping (Int) -> Void = { _ in },
        encodeGPX: (@Sendable (Trip) -> Data)? = nil,
        uploadTiming: TripUploadModel.Timing = TripUploadModel.Timing()
    ) {
        self.model = model
        self.tripID = tripID
        self.onClose = onClose
        self.onOpenRide = onOpenRide
        self.onEvenOut = onEvenOut
        self.onEditDays = onEditDays
        self.onOpenDay = onOpenDay
        self.encodeGPX = encodeGPX
        self.uploadTiming = uploadTiming
    }

    private var editDaysButton: some View {
        Button("Edit days", action: onEditDays)
            .buttonStyle(.obcGhost)
            .padding(.top, 10)
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
                            .padding(.bottom, 6)
                        dayRows(unridden)
                        editDaysButton
                    }
                } else {
                    header
                    OBCEyebrow("Days")
                        .padding(.top, 24)
                        .padding(.bottom, 6)
                    dayRows(Array(days.indices))
                    editDaysButton
                }
                OBCGroupedSection {
                    OBCListRow(
                        label: "Start date",
                        detail: trip?.startDay == nil ? "Set it to see the date of each day." : nil,
                        value: trip?.startDay.map { OBCFormat.tripDay($0) } ?? "Not set",
                        showsChevron: true
                    ) { startDateShown = true }
                    .accessibilityIdentifier("trip.startDate")
                    OBCBikeTypeRow(type: trip?.bikeType ?? .road) { model.setTripBikeType(tripID, to: $0) }
                        .accessibilityIdentifier("trip.bikeType")
                }
                .padding(.top, 20)
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 24)
        }
        .background(OBCTheme.page.ignoresSafeArea())
        .navigationTitle("Trip")
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .toolbar {
            #if os(iOS)
            if let shareMenu {
                ToolbarItem(placement: .primaryAction) { shareMenu }
            }
            #endif
            overflowMenu
        }
        .accessibilityIdentifier("trip.screen")
        .onAppear { appearances += 1 }
        // Once per open: the first pass runs before `onAppear` counts it.
        .task(id: journalInput) {
            guard appearances > 0, let trip else { return }
            let journal = journal ?? model.tripJournal()
            self.journal = journal
            await journal.load(trip: trip, rides: journalInput.rides)
        }
        .obcRenameSheet(
            "Rename trip",
            isPresented: $renameShown,
            name: trip?.name ?? "",
            onSave: {
                let name = $0.trimmingCharacters(in: .whitespacesAndNewlines)
                if !name.isEmpty { model.renameTrip(tripID, to: name) }
            }
        )
        .obcRenameSheet(
            "Rename day",
            isPresented: Binding(get: { dayRename != nil }, set: { if !$0 { dayRename = nil } }),
            name: dayRename.flatMap { trip?.dayEnds[safe: $0]?.title } ?? "",
            onSave: {
                if let day = dayRename { model.renameTripDay(tripID, day: day, to: $0) }
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
        .fullScreenCover(isPresented: $replayShown) {
            if let replayContent { ReplayPlayerView(content: replayContent) }
        }
        #else
        .sheet(isPresented: $mapShown) { tripMapCover }
        .sheet(isPresented: $replayShown) {
            if let replayContent { ReplayPlayerView(content: replayContent) }
        }
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

    /// The whole-trip hero map: every day in its palette colour, with the trip's start and end.
    /// Tapping it, online and with geometry, opens the full-screen interactive map, the same
    /// affordance as the route detail's hero.
    private var heroMap: some View {
        expandable(MultiTrackPreviewView(stages: previewStages, showsEnds: true).frame(height: 200))
            .padding(.top, 8)
    }

    @ViewBuilder
    private func expandable(_ preview: some View) -> some View {
        if canExpandMap {
            Button { mapShown = true } label: {
                // The preview ignores hits, because the tap is ours, so make the whole hero the
                // tap target.
                preview
                    .overlay(alignment: .bottomTrailing) { mapAffordance }
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("trip.expandMap")
            .accessibilityLabel("Open map")
        } else {
            preview
                .overlay(alignment: .bottomTrailing) { mapAffordance }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(isOnline ? "Trip map unavailable" : "Trip map unavailable offline")
        }
    }

    private var mapAffordance: some View {
        Label(
            canExpandMap ? "Open map" : (isOnline ? "Map unavailable" : "Offline preview"),
            systemImage: canExpandMap ? "arrow.up.left.and.arrow.down.right" : (isOnline ? "map" : "wifi.slash")
        )
        .font(.system(.subheadline, weight: .medium))
        .foregroundStyle(OBCTheme.ink)
        .padding(10)
        .background(OBCTheme.surface, in: Capsule())
        .padding(10)
        .accessibilityHidden(true)
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
        let longest = days.map(\.distanceMeters).max() ?? 0
        let copies = dayCopyNotes
        return OBCGroupedSection {
            ForEach(indices, id: \.self) { index in
                let day = days[index]
                let end = trip?.dayEnds[safe: index]
                let transfer = trip?.transferMeters(after: index)
                HStack(spacing: 0) {
                    TripDayRow(
                        color: OBCTheme.stageColor(index: index),
                        number: index + 1,
                        title: end?.title ?? end?.name.map { "to \($0)" },
                        detail: [
                            dates[safe: index].flatMap { $0 }.map { OBCFormat.tripDay($0) },
                            OBCFormat.distance(meters: day.distanceMeters), OBCFormat.climb(meters: day.elevationGainMeters),
                            day.estimatedDuration.map { OBCFormat.movingTime($0) + " h" }, offLineNote(end),
                        ].compactMap { $0 }.joined(separator: " · "),
                        note: copies[safe: index]?.shown,
                        fraction: longest > 0 ? day.distanceMeters / longest : 0,
                        spokenNote: copies[safe: index]?.spoken,
                        showsDivider: false
                    ) {
                        onOpenDay(index)
                    }
                    .accessibilityIdentifier("trip.day.\(index)")
                    Menu { dayActions(index) } label: {
                        Image(systemName: "ellipsis")
                            .font(.system(.subheadline, weight: .semibold))
                            .foregroundStyle(OBCTheme.secondary)
                            .frame(width: 44, height: 44)
                            .contentShape(Rectangle())
                    }
                    .accessibilityLabel("Day \(index + 1) actions")
                    .accessibilityIdentifier("trip.day.\(index).menu")
                    .padding(.trailing, 4)
                }
                .contextMenu { dayActions(index) }
                .overlay(alignment: .bottom) {
                    if index != indices.last && transfer == nil {
                        OBCTheme.hairline.frame(height: 1).padding(.leading, 16)
                    }
                }
                if let meters = transfer {
                    TripTransferRow(kind: end?.transfer, meters: meters) {
                        model.setTripTransfer(tripID, day: index, to: $0)
                    }
                    .accessibilityIdentifier("trip.transfer.\(index)")
                }
            }
        }
    }

    @ViewBuilder
    private func dayActions(_ index: Int) -> some View {
        Button { dayRename = index } label: { Label("Rename day", systemImage: "pencil") }
        if index < days.count - 1 {
            Button {
                stopsModel = model.tripStops(tripID, day: index, isOnline: isOnline)
            } label: { Label("End day at a stop", systemImage: "tent") }
            .accessibilityIdentifier("trip.day.stops")
        }
    }

    /// What the device holds of each day. The row shows it only while the trip on the device is
    /// out of date, where it says which days changed; VoiceOver always reads it.
    private var dayCopyNotes: [(shown: String?, spoken: String)] {
        let name = model.deviceName
        let showsDays = model.tripOnDeviceState(tripID) == .outdated
        return model.tripDayOnDeviceStates(tripID).map { state in
            let line = switch state {
            case .upToDate: "On \(name)"
            case .outdated: "Changed since it was sent"
            case .notOnDevice: "Not on \(name) yet"
            }
            return (showsDays && state != .upToDate ? line : nil, line)
        }
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
        let stages = journal.runs.map(MultiTrackPreviewView.Stage.init)
        let transfers = journal.transfers.keys.sorted().compactMap { day -> MultiTrackPreviewView.Pin? in
            guard let start = trip.dayStart(day + 1)?.coordinate else { return nil }
            let end = trip.dayEnds[day].coordinate
            return MultiTrackPreviewView.Pin(
                coordinate: Coordinate(
                    latitude: (end.latitude + start.latitude) / 2, longitude: (end.longitude + start.longitude) / 2),
                color: OBCTheme.secondary, systemImage: transferKind(day)?.systemImage ?? "arrow.right")
        }
        let stopped = journal.review?.days.last { $0.endedAt != nil }?.endedAt.map {
            MultiTrackPreviewView.Pin(coordinate: trip.measuredLine.coordinate(at: $0), color: OBCTheme.ink)
        }
        let pins = trip.dayEnds.dropLast().map { MultiTrackPreviewView.Pin(coordinate: $0.coordinate, color: OBCTheme.ink) }
            + journal.photoPins.map { MultiTrackPreviewView.Pin(coordinate: $0, color: OBCTheme.ride) }
            + transfers + [stopped].compactMap { $0 }
        return expandable(MultiTrackPreviewView(stages: stages, pins: pins).frame(height: 230))
            .padding(.top, 8)
            .accessibilityIdentifier("trip.journal.map")
    }

    /// The label of the transfer after `day`, as the rider set it last.
    private func transferKind(_ day: Int) -> TransferKind? {
        trip?.dayEnds[safe: day]?.transfer
    }

    /// "Tue 29 Sep – Fri 2 Oct · day 3 of 4", or "Day 3 of 4" when the days have no dates.
    private func journalDateLine(_ review: TripReview, _ shown: Trip) -> String {
        let progress = review.currentDay.map { "day \($0 + 1) of \(shown.dayCount)" }
        let dateLine = [model.tripDateLine(tripID), progress].compactMap { $0 }.joined(separator: " · ")
        return model.tripDateLine(tripID) == nil ? dateLine.prefix(1).uppercased() + dateLine.dropFirst() : dateLine
    }

    private func journalHeader(_ journal: TripJournalModel, _ review: TripReview, _ shown: Trip) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            journalMap(journal, shown)
            titleRow(trip?.name ?? shown.name)
            Text(journalDateLine(review, shown))
                .font(.system(.subheadline).monospacedDigit())
                .foregroundStyle(OBCTheme.secondary)
                .padding(.top, 3)
            deviceStatus
                .padding(.top, 14)
            TripReviewTotals(
                ridden: review.totals, plannedMeters: review.plannedMeters, isDone: review.currentDay == nil,
                highlights: journal.highlights
            )
            .padding(.top, 18)
            .accessibilityIdentifier("trip.journal.totals")
            if journal.canReplay {
                Button("Replay ridden days", systemImage: "play.circle") {
                    replayPreparing = true
                    Task {
                        replayContent = await journal.replayContent()
                        replayPreparing = false
                        replayShown = replayContent != nil
                    }
                }
                .buttonStyle(.obcGhost)
                .disabled(replayPreparing)
                .padding(.top, 16)
                .accessibilityIdentifier("trip.journal.replay")
            }
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
        VStack(alignment: .leading, spacing: 0) {
            heroMap
            titleRow(trip?.name ?? "Trip")
            if let dates = model.tripDateLine(tripID) {
                Text(dates)
                    .font(.system(.subheadline).monospacedDigit())
                    .foregroundStyle(OBCTheme.secondary)
                    .padding(.top, 3)
                    .accessibilityIdentifier("trip.dates")
            }
            deviceStatus
                .padding(.top, 14)
            OBCLedger(ledger)
                .padding(.top, 20)
                .accessibilityIdentifier("trip.stats")
        }
    }

    /// The name with its rename button, as on the route page.
    private func titleRow(_ name: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 4) {
            Text(name)
                .font(.system(.title, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityAddTraits(.isHeader)
                .accessibilityIdentifier("trip.title")
            Button { renameShown = true } label: {
                Image(systemName: "pencil")
                    .font(.system(.body, weight: .medium))
                    .foregroundStyle(OBCTheme.secondary)
                    .obcFixedGeometryType()
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            // The glyph's optical edge lines up with the page margin.
            .padding(.trailing, -12)
            .accessibilityLabel("Rename trip")
            .accessibilityIdentifier("trip.rename")
        }
        .padding(.top, 14)
    }

    /// DISTANCE, CLIMB, DAYS, and RIDING TIME when every day has an estimate.
    private var ledger: [OBCStat] {
        let stats = model.tripStats(tripID)
        let estimates = days.compactMap(\.estimatedDuration)
        return [
            OBCStat(value: OBCFormat.distanceValue(meters: stats.distanceMeters), unit: "km", key: "Distance"),
            OBCStat(value: OBCFormat.climbValue(meters: stats.elevationGainMeters), unit: "m", key: "Climb"),
            OBCStat(value: "\(stats.dayCount)", key: "Days"),
        ] + (estimates.count == days.count && !days.isEmpty
            ? [OBCStat(value: OBCFormat.movingTime(estimates.reduce(0, +)), unit: "h", key: "Riding time")] : [])
    }

    /// What the device holds of the trip, and the one action. The tap re-reads the device catalogs
    /// first, so a retry after a failed upload plans against what actually landed and never mints a
    /// duplicate from a pre-failure cache.
    private var deviceStatus: some View {
        DeviceCopyStatus(
            state: model.tripOnDeviceState(tripID), connection: model.connection, deviceName: model.deviceName,
            idPrefix: "trip"
        ) {
            guard !isPreparingUpload else { return }
            isPreparingUpload = true
            Task {
                tripUploadModel = await model.prepareTripUpload(tripID, timing: uploadTiming)
                isPreparingUpload = false
            }
        }
    }

    #if os(iOS)
    private var shareMenu: ShareMenu? {
        guard let encodeGPX, let journal, let review = journal.review, let shown = journal.trip else { return nil }
        return ShareMenu(
            gpx: GPXFile(name: shown.name) { encodeGPX(shown) },
            image: ShareCardContent(
                trip: shown, review: review, runs: journal.runs, dateLine: journalDateLine(review, shown)))
    }
    #endif

    // MARK: Overflow

    private var overflowMenu: some ToolbarContent {
        ToolbarItem(placement: .primaryAction) {
            Menu {
                Button { reverseDialogShown = true } label: {
                    Label("Reverse trip…", systemImage: "arrow.left.arrow.right")
                }
                .accessibilityIdentifier("trip.reverse")

                Divider()

                Button(role: .destructive) { deleteDialogShown = true } label: {
                    Label("Delete trip…", systemImage: "trash")
                }
                .accessibilityIdentifier("trip.delete")
            } label: {
                Image(systemName: "ellipsis")
            }
            .accessibilityLabel("More")
            .accessibilityIdentifier("trip.overflow")
            .confirmationDialog(
                "Reverse this trip?", isPresented: $reverseDialogShown, titleVisibility: .visible
            ) {
                Button("Reverse trip") { model.reverseTrip(tripID) }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Reverses the direction and day order of this trip. Its device progress starts over.")
            }
            .obcDestructiveConfirm(
                "Delete \(trip?.name.quoted ?? "trip")?",
                isPresented: $deleteDialogShown,
                message: model.connectedScope != nil
                    ? "Removes this trip from your library. Also tries to remove its trip and day routes from \(model.deviceName)."
                    : "Removes this trip from your library. \(model.deviceName) is not connected, so its copies stay there.",
                actionTitle: "Delete trip",
                onConfirm: {
                    model.deleteTrip(tripID)
                    onClose()
                }
            )
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
