import SwiftUI
import OBCDomain

/// The day editor: the map and the profile with the day-end handles stay put, and the day list
/// scrolls under them. In split mode a stepper in the list header sets the day count; in edit
/// mode Even out sits there instead. A tap on a handle or a day row opens the stops sheet; a
/// swipe on a row removes its day end. Done saves; the back chevron with changes asks first.
public struct TripDayEditorView: View {
    /// Held here, so a host that builds the screen again on a body pass keeps the draft.
    @State private var model: TripDayEditorModel
    private let onClose: () -> Void

    @State private var discardShown = false
    @State private var stopsModel: TripStopsModel?
    @State private var dayRename: Int?
    @State private var dayDraft = ""

    @Environment(\.obcIsOnline) private var isOnline

    /// `onClose` pops the screen, after Done saved or Discard let the draft go.
    public init(model: TripDayEditorModel, onClose: @escaping () -> Void) {
        _model = State(initialValue: model)
        self.onClose = onClose
    }

    private var trip: Trip { model.trip }

    public var body: some View {
        VStack(spacing: 0) {
            LineMarkerEditor(model: model.handles, mapHeight: 164, profileHeight: 160)
                .padding(.horizontal, 20)
                .padding(.top, 4)
            list
        }
        .background(OBCTheme.parchment.ignoresSafeArea())
        .navigationTitle(trip.name)
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        .navigationBarBackButtonHidden(true)
        #endif
        .toolbar { toolbar }
        .onAppear { model.handles.onTap = { [weak model] id in
            guard let model, let day = model.day(of: id) else { return }
            stopsModel = model.stops(for: day, isOnline: isOnline)
        } }
        .confirmationDialog("Discard changes?", isPresented: $discardShown, titleVisibility: .visible) {
            Button("Discard changes", role: .destructive, action: onClose)
            Button("Keep editing", role: .cancel) {}
        }
        .obcRenameAlert(
            "Rename day",
            isPresented: Binding(get: { dayRename != nil }, set: { if !$0 { dayRename = nil } }),
            name: $dayDraft,
            onSave: { if let day = dayRename { model.renameDay(day, to: dayDraft) } }
        )
        .sheet(item: $stopsModel) { stops in
            TripStopsSheet(model: stops)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
        }
        .accessibilityIdentifier("dayEditor.screen")
    }

    // MARK: Toolbar

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            Button {
                if model.hasChanges { discardShown = true } else { onClose() }
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
                    onClose()
                }
                .fontWeight(.semibold)
                .accessibilityIdentifier("dayEditor.done")
            }
        }
    }

    // MARK: List

    private var list: some View {
        List {
            Section {
                ForEach(0..<trip.dayCount, id: \.self) { day in
                    dayRow(day)
                }
            } header: {
                VStack(alignment: .leading, spacing: 8) {
                    header
                    if model.isSplitMode, model.balancesByDistance {
                        Text("This file has no elevation. Days are balanced by distance.")
                            .font(.system(size: 13))
                            .foregroundStyle(OBCTheme.inkSoft)
                    }
                }
                .textCase(nil)
                .padding(.bottom, 2)
            }
            .listRowBackground(OBCTheme.panel)
            .listRowSeparator(.hidden)
            .listRowInsets(EdgeInsets())

            Section {
                quietRow("Add day end", systemImage: "plus", id: "addDayEnd") { model.addDayEnd() }
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
        .contentMargins(.top, 14, for: .scrollContent)
        .animation(.snappy(duration: 0.28), value: trip.dayCount)
    }

    /// "4 DAYS · ~5 H A DAY", with the stepper in split mode and Even out while editing.
    private var header: some View {
        HStack {
            let hours = Int((model.averageDayDuration / 3600).rounded())
            OBCEyebrow("\(trip.dayCount) \(trip.dayCount == 1 ? "day" : "days") · ~\(hours) h a day")
                .accessibilityIdentifier("dayEditor.summary")
            Spacer()
            if model.isSplitMode {
                Stepper(
                    "Days",
                    value: Binding(get: { trip.dayCount }, set: { model.setDayCount($0) }),
                    in: 1...Trip.maxSplitDays
                )
                .labelsHidden()
                .accessibilityIdentifier("dayEditor.days")
            } else {
                Button("Even out") { model.evenOut() }
                    .font(.system(size: 15))
                    .foregroundStyle(OBCTheme.forest)
                    .accessibilityIdentifier("dayEditor.evenOut")
            }
        }
    }

    /// One day: its colour, number, name and live figures. The last day ends the trip, so it
    /// opens no stops sheet and has no swipe.
    private func dayRow(_ day: Int) -> some View {
        let end = trip.dayEnds[day]
        let stats = model.stats.indices.contains(day) ? model.stats[day] : nil
        let isLast = day == trip.dayCount - 1
        let blocker = model.removeBlocker(day)
        return TripDayRow(
            color: OBCTheme.stageColor(index: day),
            number: day + 1,
            title: end.title ?? end.name.map { "to \($0)" },
            detail: detail(day, stats: stats, isTransfer: !isLast && blocker != nil),
            showsDivider: !isLast
        ) {
            guard !isLast else { return rename(day) }
            stopsModel = model.stops(for: day, isOnline: isOnline)
        }
        .contextMenu {
            Button { rename(day) } label: { Label("Rename day", systemImage: "pencil") }
            if blocker == nil {
                Button(role: .destructive) { model.removeDayEnd(day) } label: {
                    Label("Remove day end", systemImage: "minus.circle")
                }
            }
        }
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

    /// "Sat · 83.5 km · 442 m ↑ · ~5:11", with the weekday when the trip has a date, and the
    /// reason a day end stays put when it ends at a transfer.
    private func detail(_ day: Int, stats: DayStats?, isTransfer: Bool) -> String {
        let date = trip.dayDates()[day].map { OBCFormat.tripDay($0) }
        let figures = stats.map {
            [OBCFormat.distance(meters: $0.distanceMeters), OBCFormat.climb(meters: $0.climbMeters),
             "~" + OBCFormat.movingTime($0.duration)].joined(separator: " · ")
        }
        return [date, figures, isTransfer ? "transfer" : nil].compactMap { $0 }.joined(separator: " · ")
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
        dayDraft = trip.dayEnds[day].title ?? ""
        dayRename = day
    }
}
