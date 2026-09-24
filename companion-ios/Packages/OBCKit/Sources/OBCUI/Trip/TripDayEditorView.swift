import SwiftUI
import OBCDomain

/// The day editor: the map is the screen and pans and zooms freely; a three-detent sheet holds
/// the profile and the day list. The profile shows the stretch the map shows, and a day end
/// moves only by a drag of its pin on the profile. A stop's map callout ends the nearest day
/// there. Each day's ··· menu, or a long press on its row, holds the day's actions. In split
/// mode a stepper in the sheet header sets the day count. Done saves; the back chevron with
/// changes asks first.
public struct TripDayEditorView: View {
    private let model: TripDayEditorModel
    private let onClose: () -> Void

    @State private var sheetShown = false
    @State private var discardShown = false
    @State private var detent = PresentationDetent.medium
    /// The sheet's height over the screen bottom, for the map: an estimate until it is measured.
    @State private var sheetHeight: CGFloat = 420
    /// The lowest detent: the sheet header and nothing more.
    @State private var peekHeight: CGFloat = 120

    @Environment(\.obcIsOnline) private var isOnline

    /// The sheet's top detent stops under the toolbar, so Undo and Done stay in reach.
    private static let topDetent = PresentationDetent.fraction(0.82)

    /// The host owns `model` for the screen's life: a body pass must not build it again.
    public init(model: TripDayEditorModel, onClose: @escaping () -> Void) {
        self.model = model
        self.onClose = onClose
    }

    private var trip: Trip { model.trip }

    public var body: some View {
        map
            .ignoresSafeArea(edges: .bottom)
            .navigationTitle(trip.name)
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            .navigationBarBackButtonHidden(true)
            #endif
            .toolbar { toolbar }
            .onAppear {
                sheetShown = true
                model.loadStops()
            }
            .onDisappear { sheetShown = false }
            .sheet(isPresented: $sheetShown) {
                DayEditorSheet(
                    model: model, discardShown: $discardShown, sheetHeight: $sheetHeight, peekHeight: $peekHeight,
                    onClose: close
                )
                .presentationDetents([.height(peekHeight), .medium, Self.topDetent], selection: $detent)
                .presentationBackgroundInteraction(.enabled(upThrough: Self.topDetent))
                // A swipe on the day list scrolls it; the grab handle and the header resize.
                .presentationContentInteraction(.scrolls)
                .presentationBackground(OBCTheme.parchment)
                .presentationDragIndicator(.visible)
                .interactiveDismissDisabled()
            }
    }

    private func close() {
        sheetShown = false
        onClose()
    }

    // MARK: Map

    @ViewBuilder
    private var map: some View {
        let handles = model.handles
        #if canImport(UIKit) && canImport(MapKit)
        if isOnline {
            LineMarkerMapView(
                model: handles, lineVersion: handles.lineVersion, markers: handles.markers,
                activeID: handles.activeID, segmentColors: handles.segmentColors,
                dashedSegments: handles.dashedSegments, stops: handles.stops,
                branches: handles.branches, oldSections: handles.oldSections,
                bottomInset: sheetHeight, linksProfile: true
            )
        } else {
            grid
        }
        #else
        grid
        #endif
    }

    /// The offline map: the days as stages of the shared grid preview.
    private var grid: some View {
        let handles = model.handles
        let bounds = [0] + handles.markers.map(\.distance) + [handles.line.length]
        let stages = (0..<(bounds.count - 1)).map { segment in
            let from = bounds[segment], to = bounds[segment + 1]
            let first = handles.line.index(at: from), last = handles.line.index(at: to)
            let inner = first + 1 <= last ? handles.line.vertices[(first + 1)...last].map(\.coordinate) : []
            return MultiTrackPreviewView.Stage(
                coordinates: [handles.line.coordinate(at: from)] + inner + [handles.line.coordinate(at: to)],
                color: handles.segmentColors[segment]
            )
        }
        return MultiTrackPreviewView(stages: stages, showsChrome: false)
            .padding(.bottom, sheetHeight)
    }

    // MARK: Toolbar

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            Button {
                if model.hasChanges { discardShown = true } else { close() }
            } label: {
                Label("Back", systemImage: "chevron.backward")
            }
            .accessibilityIdentifier("dayEditor.back")
        }
        ToolbarItem(placement: .primaryAction) {
            HStack(spacing: 18) {
                Button { model.undo() } label: { Label("Undo", systemImage: "arrow.uturn.backward") }
                    .disabled(!model.canUndo)
                    .accessibilityIdentifier("dayEditor.undo")
                Button("Done") {
                    model.save()
                    close()
                }
                .fontWeight(.semibold)
                .disabled(model.bridging != nil)
                .accessibilityIdentifier("dayEditor.done")
            }
        }
    }
}

/// The sheet over the map: a one-line header with the trip's figures and, in split mode, the
/// stepper; the profile; and the day rows. The alerts and sheets of the editor present from
/// here, above the sheet.
struct DayEditorSheet: View {
    let model: TripDayEditorModel
    @Binding var discardShown: Bool
    /// Reported to the host: the sheet's height, and its lowest detent, the header's height.
    @Binding var sheetHeight: CGFloat
    @Binding var peekHeight: CGFloat
    let onClose: () -> Void

    @State private var stopsModel: TripStopsModel?
    /// The choice for a stop off the line, shown once no other sheet is up.
    @State private var offLine: OffLineStopModel?
    @State private var dayRename: Int?

    @Environment(\.obcIsOnline) private var isOnline

    private var trip: Trip { model.trip }

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(.horizontal, 20)
                .padding(.top, 20)
                .padding(.bottom, 12)
                .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { peekHeight = $0 }
            if model.isSplitMode, model.balancesByDistance {
                Text("This file has no elevation. Days are balanced by distance.")
                    .font(.system(size: 13))
                    .foregroundStyle(OBCTheme.inkSoft)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 20)
                    .padding(.bottom, 10)
            }
            LineMarkerProfileView(model: model.handles, height: 150, showsAxis: true)
                .padding(.horizontal, 20)
            list
        }
        // A short sheet cuts the bottom, never the header: without the zero minimum the frame
        // grows to the content and the sheet centres it.
        .frame(minHeight: 0, maxHeight: .infinity, alignment: .top)
        .clipped()
        .background(OBCTheme.parchment)
        .onGeometryChange(for: CGFloat.self) { $0.size.height + $0.safeAreaInsets.bottom } action: { sheetHeight = $0 }
        .alert("Discard changes?", isPresented: $discardShown) {
            Button("Discard", role: .destructive, action: onClose)
            Button("Keep Editing", role: .cancel) {}
        } message: {
            Text("Your day ends go back to the saved trip.")
        }
        .obcRenameSheet(
            "Rename day",
            isPresented: Binding(get: { dayRename != nil }, set: { if !$0 { dayRename = nil } }),
            name: dayRename.flatMap { trip.dayEnds.indices.contains($0) ? trip.dayEnds[$0].title : nil } ?? "",
            onSave: { if let day = dayRename { model.renameDay(day, to: $0) } }
        )
        .sheet(item: $stopsModel, onDismiss: { offLine = model.offLineStop }) { stops in
            TripStopsSheet(model: stops)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
        }
        .onChange(of: model.offLineStop?.id) {
            if stopsModel == nil { offLine = model.offLineStop }
        }
        .sheet(item: $offLine, onDismiss: { model.offLineStop = nil }) { choice in
            OffLineStopSheet(model: choice, color: OBCTheme.stageColor(index: choice.day))
                .presentationDetents([.height(290)])
                .presentationDragIndicator(.visible)
        }
        .alert(
            model.bridgeFailure.map { $0 == .noRoad ? "No road across the gap found." : OffLineStopSheet.message($0) } ?? "",
            isPresented: Binding(get: { model.bridgeFailure != nil }, set: { if !$0 { model.bridgeFailure = nil } })
        ) {
            Button("OK", role: .cancel) {}
        } message: {
            Text("The day rides it as a straight line.")
        }
    }

    // MARK: Header

    /// "4 days · 324 km · ~5 h a day", with the stepper beside it in split mode. The lowest
    /// detent is this line: it holds no button, and one height fits both modes.
    private var header: some View {
        HStack(spacing: 12) {
            let hours = Int((model.averageDayDuration / 3600).rounded())
            Text(
                "\(trip.dayCount) \(trip.dayCount == 1 ? "day" : "days") · "
                    + "\(OBCFormat.distance(meters: model.handles.line.length)) · "
                    + (hours < 1 ? "under 1 h a day" : "~\(hours) h a day"))
                .font(.system(size: 15, weight: .semibold))
                .monospacedDigit()
                .foregroundStyle(OBCTheme.ink)
                .lineLimit(1)
                .minimumScaleFactor(0.8)
                .accessibilityIdentifier("dayEditor.summary")
            Spacer(minLength: 0)
            if model.isSplitMode {
                Stepper(
                    "Days",
                    value: Binding(get: { trip.dayCount }, set: { model.setDayCount($0) }),
                    in: 1...model.maxDays
                )
                .labelsHidden()
                .accessibilityIdentifier("dayEditor.days")
            }
        }
        .frame(minHeight: 32)
    }

    // MARK: Rows

    private var list: some View {
        List {
            ForEach(0..<trip.dayCount, id: \.self) { day in
                dayRow(day)
                    .listRowBackground(model.selectedDay == day ? OBCTheme.parchment2 : OBCTheme.panel)
            }
            .listRowSeparator(.hidden)
            .listRowInsets(EdgeInsets())
        }
        #if os(iOS)
        .listStyle(.insetGrouped)
        #endif
        .scrollContentBackground(.hidden)
        .contentMargins(.top, 12, for: .scrollContent)
        .animation(.snappy(duration: 0.28), value: trip.dayCount)
    }

    /// One day: its colour, number, name and figures, and its menu. A tap highlights it.
    private func dayRow(_ day: Int) -> some View {
        let end = trip.dayEnds[day]
        let isLast = day == trip.dayCount - 1
        return HStack(spacing: 0) {
            DayFigureRow(
                model: model, day: day,
                title: end.title ?? end.name.map { "to \($0)" },
                date: trip.dayDates()[day].map { OBCFormat.tripDay($0) },
                note: !isLast && model.removeBlocker(day) != nil ? "transfer" : nil,
                route: model.notes.indices.contains(day) ? model.notes[day] : nil,
                showsDivider: !isLast
            ) {
                model.select(day)
            }
            .accessibilityIdentifier("dayEditor.day.\(day)")
            Menu { dayActions(day) } label: {
                Image(systemName: "ellipsis")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(OBCTheme.inkSoft)
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("Day \(day + 1) actions")
            .accessibilityIdentifier("dayEditor.day.\(day).menu")
            .padding(.trailing, 6)
        }
        .contextMenu { dayActions(day) }
    }

    /// End at a stop, Rename, Split this day, Join with the next day: the ··· menu and the
    /// row's long press. A day end at a transfer holds a gap, so it neither moves to a stop nor
    /// joins.
    @ViewBuilder
    private func dayActions(_ day: Int) -> some View {
        let movable = model.removeBlocker(day) == nil
        if movable {
            Button { stopsModel = model.stops(for: day, isOnline: isOnline) } label: {
                Label("End at a stop", systemImage: "tent")
            }
            .accessibilityIdentifier("dayEditor.day.stops")
        }
        Button { dayRename = day } label: { Label("Rename", systemImage: "pencil") }
            .accessibilityIdentifier("dayEditor.day.rename")
        if model.canSplit(day) {
            Button { model.splitDay(day) } label: { Label("Split this day", systemImage: "scissors") }
                .accessibilityIdentifier("dayEditor.day.split")
        }
        if model.canBridge(day) {
            Button { model.bridgeGap(in: day) } label: { Label("Bridge the gap", systemImage: "point.topleft.down.to.point.bottomright.curvepath") }
                .accessibilityIdentifier("dayEditor.day.bridge")
        }
        if movable {
            Button { model.joinDay(day) } label: {
                Label("Join with Day \(day + 2)", systemImage: "arrow.merge")
            }
            .accessibilityIdentifier("dayEditor.day.join")
        }
    }
}

/// A day row that reads the live figures of a drag, so a frame re-renders this row alone.
private struct DayFigureRow: View {
    let model: TripDayEditorModel
    let day: Int
    let title: String?
    let date: String?
    let note: String?
    /// How the day reaches its stop or rides a gap.
    let route: String?
    let showsDivider: Bool
    let action: () -> Void

    var body: some View {
        let stats = model.live.days[day] ?? (model.stats.indices.contains(day) ? model.stats[day] : nil)
        let figures = stats.map {
            [OBCFormat.distance(meters: $0.distanceMeters), OBCFormat.climb(meters: $0.climbMeters),
             "~" + OBCFormat.movingTime($0.duration)].joined(separator: " · ")
        }
        TripDayRow(
            color: OBCTheme.stageColor(index: day),
            number: day + 1,
            title: title,
            detail: [date, figures, note].compactMap { $0 }.joined(separator: " · "),
            note: route,
            showsDivider: showsDivider,
            action: action
        )
    }
}
