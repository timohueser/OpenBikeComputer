import SwiftUI
import OBCDomain

/// The day editor: the map is the screen and pans and zooms freely; a three-detent sheet holds
/// the profile and the day list. Pins on the map and the profile are the primary control. In
/// split mode a stepper in the sheet header sets the day count; in edit mode Even out sits
/// there. A tap on a pin or a long press on a row offers End the day at a stop, Rename and
/// Remove; a tap on a row focuses that day end. "Add day end" arms placement: the next tap on
/// the line, on the map or the profile, puts the new end there. Done saves; the back chevron
/// with changes asks first.
public struct TripDayEditorView: View {
    private let model: TripDayEditorModel
    private let onClose: () -> Void

    @State private var sheetShown = false
    @State private var discardShown = false
    @State private var dayMenu: Int?

    @Environment(\.obcIsOnline) private var isOnline

    /// How much of the screen the sheet covers at its resting detent: the map fits above it.
    private static let sheetInset: CGFloat = 330
    /// The sheet's top detent stops under the toolbar, so Undo and Done stay in reach.
    private static let topDetent = PresentationDetent.fraction(0.82)

    /// The host owns `model` for the screen's life: a body pass must not build it again.
    public init(model: TripDayEditorModel, onClose: @escaping () -> Void) {
        self.model = model
        self.onClose = onClose
    }

    private var trip: Trip { model.trip }

    public var body: some View {
        ZStack(alignment: .top) {
            map.ignoresSafeArea(edges: .bottom)
            if model.isPlacing {
                Text("Tap the line where the new day ends")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(OBCTheme.parchment)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 9)
                    .background(OBCTheme.ink.opacity(0.9), in: Capsule())
                    .padding(.top, 10)
                    .transition(.opacity)
                    .accessibilityIdentifier("dayEditor.placementHint")
            }
        }
        .animation(.snappy(duration: 0.2), value: model.isPlacing)
        .navigationTitle(trip.name)
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        .navigationBarBackButtonHidden(true)
        #endif
        .toolbar { toolbar }
        .onAppear {
            sheetShown = true
            model.loadStops()
            model.handles.onTap = { [weak model] id in dayMenu = model?.day(of: id) }
        }
        .onDisappear { sheetShown = false }
        .sheet(isPresented: $sheetShown) {
            DayEditorSheet(model: model, discardShown: $discardShown, dayMenu: $dayMenu, onClose: close)
                .presentationDetents([.fraction(0.24), .medium, Self.topDetent], selection: .constant(.medium))
                .presentationBackgroundInteraction(.enabled(upThrough: Self.topDetent))
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
                bottomInset: Self.sheetInset, isPlacing: handles.isPlacing
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
            .padding(.bottom, Self.sheetInset)
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
                .accessibilityIdentifier("dayEditor.done")
            }
        }
    }
}

/// The sheet over the map: the header with the stepper or Even out, the zoomable profile with
/// its overview strip, and the day rows. The alerts and sheets of the editor present from
/// here, above the sheet.
struct DayEditorSheet: View {
    let model: TripDayEditorModel
    @Binding var discardShown: Bool
    @Binding var dayMenu: Int?
    let onClose: () -> Void

    @State private var stopsModel: TripStopsModel?
    @State private var dayRename: Int?

    @Environment(\.obcIsOnline) private var isOnline

    private var trip: Trip { model.trip }

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(.horizontal, 20)
                .padding(.top, 18)
            LineMarkerProfileView(model: model.handles, height: 150, showsOverview: true)
                .padding(.horizontal, 20)
                .padding(.top, 12)
            list
        }
        .background(OBCTheme.parchment)
        .alert("Discard changes?", isPresented: $discardShown) {
            Button("Discard", role: .destructive, action: onClose)
            Button("Keep Editing", role: .cancel) {}
        } message: {
            Text("Your day ends go back to the saved trip.")
        }
        .confirmationDialog(
            dayMenu.map(dayTitle) ?? "", isPresented: Binding(get: { dayMenu != nil }, set: { if !$0 { dayMenu = nil } }),
            titleVisibility: .visible
        ) {
            if let day = dayMenu { dayActions(day) }
        }
        .obcRenameSheet(
            "Rename day",
            isPresented: Binding(get: { dayRename != nil }, set: { if !$0 { dayRename = nil } }),
            name: dayRename.flatMap { trip.dayEnds.indices.contains($0) ? trip.dayEnds[$0].title : nil } ?? "",
            onSave: { if let day = dayRename { model.renameDay(day, to: $0) } }
        )
        .sheet(item: $stopsModel) { stops in
            TripStopsSheet(model: stops)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
        }
    }

    // MARK: Header

    /// "4 DAYS · ~5 H A DAY" with the stepper in split mode and Even out while editing; while a
    /// day end is placed, the hint and Cancel.
    @ViewBuilder
    private var header: some View {
        if model.isPlacing {
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    OBCEyebrow("New day end")
                    Text("Tap the line on the map or the profile.")
                        .font(.system(size: 14))
                        .foregroundStyle(OBCTheme.inkSoft)
                }
                Spacer()
                Button("Cancel") { model.cancelPlacing() }
                    .font(.system(size: 16))
                    .foregroundStyle(OBCTheme.forest)
                    .accessibilityIdentifier("dayEditor.cancelPlacing")
            }
        } else {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    let hours = Int((model.averageDayDuration / 3600).rounded())
                    OBCEyebrow("\(trip.dayCount) \(trip.dayCount == 1 ? "day" : "days") · ~\(hours) h a day")
                        .accessibilityIdentifier("dayEditor.summary")
                    Spacer()
                    if model.isSplitMode {
                        Stepper(
                            "Days",
                            value: Binding(get: { trip.dayCount }, set: { model.setDayCount($0) }),
                            in: 1...model.maxDays
                        )
                        .labelsHidden()
                        .accessibilityIdentifier("dayEditor.days")
                    } else {
                        Button("Even out") { model.evenOut() }
                            .font(.system(size: 16))
                            .foregroundStyle(OBCTheme.forest)
                            .accessibilityIdentifier("dayEditor.evenOut")
                    }
                }
                if model.isSplitMode, model.balancesByDistance {
                    Text("This file has no elevation. Days are balanced by distance.")
                        .font(.system(size: 13))
                        .foregroundStyle(OBCTheme.inkSoft)
                }
            }
        }
    }

    // MARK: Rows

    private var list: some View {
        List {
            Section {
                ForEach(0..<trip.dayCount, id: \.self) { day in
                    dayRow(day)
                }
            }
            .listRowBackground(OBCTheme.panel)
            .listRowSeparator(.hidden)
            .listRowInsets(EdgeInsets())

            Section {
                quietRow("Add day end", systemImage: "plus", id: "addDayEnd") { model.beginPlacing() }
                    .disabled(model.isPlacing)
                if model.isSplitMode {
                    quietRow("Even out days", systemImage: "arrow.left.and.right", id: "evenOut") { model.evenOut() }
                }
            }
            .listRowBackground(OBCTheme.panel)
            .listRowSeparatorTint(OBCTheme.screenLine)
            .listRowInsets(EdgeInsets())
        }
        #if os(iOS)
        .listStyle(.insetGrouped)
        #endif
        .scrollContentBackground(.hidden)
        .contentMargins(.top, 12, for: .scrollContent)
        .animation(.snappy(duration: 0.28), value: trip.dayCount)
    }

    /// One day: its colour, number, name and figures. A tap focuses its end; the last day ends
    /// the trip, so it has no swipe.
    private func dayRow(_ day: Int) -> some View {
        let end = trip.dayEnds[day]
        let isLast = day == trip.dayCount - 1
        let blocker = model.removeBlocker(day)
        return DayFigureRow(
            model: model, day: day,
            title: end.title ?? end.name.map { "to \($0)" },
            date: trip.dayDates()[day].map { OBCFormat.tripDay($0) },
            note: !isLast && blocker != nil ? "transfer" : nil,
            showsDivider: !isLast
        ) {
            model.select(isLast ? nil : day)
        }
        .contextMenu { dayActions(day) }
        .swipeActions(edge: .trailing, allowsFullSwipe: blocker == nil) {
            if blocker == nil {
                Button(role: .destructive) { model.removeDayEnd(day) } label: {
                    Label("Remove", systemImage: "minus.circle")
                }
                .tint(OBCTheme.warning)
            }
        }
        .accessibilityIdentifier("dayEditor.day.\(day)")
    }

    /// End the day at a stop, Rename day, Remove day end: the pin menu and the row's long press.
    @ViewBuilder
    private func dayActions(_ day: Int) -> some View {
        if day < trip.dayCount - 1 {
            Button { stopsModel = model.stops(for: day, isOnline: isOnline) } label: {
                Label("End the day at a stop", systemImage: "tent")
            }
            .accessibilityIdentifier("dayEditor.day.stops")
        }
        Button { rename(day) } label: { Label("Rename day", systemImage: "pencil") }
            .accessibilityIdentifier("dayEditor.day.rename")
        if model.removeBlocker(day) == nil {
            Button(role: .destructive) { model.removeDayEnd(day) } label: {
                Label("Remove day end", systemImage: "minus.circle")
            }
            .accessibilityIdentifier("dayEditor.day.remove")
        }
    }

    /// "Day 2 · to Bad Tabarz"
    private func dayTitle(_ day: Int) -> String {
        guard trip.dayEnds.indices.contains(day) else { return "" }
        let end = trip.dayEnds[day]
        return ["Day \(day + 1)", end.title ?? end.name.map { "to \($0)" }].compactMap { $0 }.joined(separator: " · ")
    }

    private func quietRow(_ title: String, systemImage: String, id: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Image(systemName: systemImage)
                    .font(.system(size: 15, weight: .medium))
                    .frame(width: 18)
                Text(title)
                    .font(.system(size: 16))
                Spacer(minLength: 0)
            }
            .foregroundStyle(OBCTheme.forest)
            .padding(.horizontal, 16)
            .frame(minHeight: 48)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("dayEditor.\(id)")
    }

    private func rename(_ day: Int) {
        dayRename = day
    }
}

/// A day row that reads the live figures of a drag, so a frame re-renders this row alone.
private struct DayFigureRow: View {
    let model: TripDayEditorModel
    let day: Int
    let title: String?
    let date: String?
    let note: String?
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
            showsDivider: showsDivider,
            action: action
        )
    }
}
