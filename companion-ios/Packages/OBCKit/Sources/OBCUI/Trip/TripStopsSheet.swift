import SwiftUI
import OBCDomain

/// The stops of one day end: the campsites, hotels and waypoints near it in ride order, with a
/// rule where the day ends now, and a search for any other place. A tap on a stop ends the day
/// there and closes the sheet.
public struct TripStopsSheet: View {
    @Bindable var model: TripStopsModel
    var onDone: () -> Void

    @Environment(\.dismiss) private var dismiss

    public init(model: TripStopsModel, onDone: @escaping () -> Void = {}) {
        self.model = model
        self.onDone = onDone
    }

    private var isSearching: Bool {
        !model.query.trimmingCharacters(in: .whitespaces).isEmpty && model.results != nil
    }

    public var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                header
                if model.endsAtTransfer { message("This day ends at a transfer.") }
                OBCSearchField(text: $model.query, prompt: "Search places")
                    .onSubmit { Task { await model.search() } }
                    .disabled(!model.canSearch)
                    .opacity(model.canSearch ? 1 : 0.45)
                    .accessibilityIdentifier("stops.search")
                if isSearching {
                    results
                } else {
                    nearby
                }
            }
            .padding(.horizontal, 20)
            .padding(.top, 20)
            .padding(.bottom, 24)
        }
        .background(OBCTheme.page.ignoresSafeArea())
        .task { await model.load() }
        .accessibilityIdentifier("stops.sheet")
    }

    private var header: some View {
        let end = model.dayEnd
        let name = end.title ?? end.name.map { "to \($0)" }
        return VStack(alignment: .leading, spacing: 3) {
            Text(["Day \(model.day + 1)", name].compactMap { $0 }.joined(separator: " · "))
                .font(.system(.title2, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
                .lineLimit(1)
            Text("End of day · km \(OBCFormat.distanceValue(meters: end.distance)) of \(OBCFormat.distanceValue(meters: model.lineLength))")
                .font(.system(.caption).monospacedDigit())
                .foregroundStyle(OBCTheme.secondary)
        }
    }

    @ViewBuilder
    private var nearby: some View {
        switch model.nearby {
        case .loading:
            ProgressView().tint(OBCTheme.secondary).frame(maxWidth: .infinity)
        case .offline:
            message("Stops need a connection.")
        case .loaded where model.stops.isEmpty:
            message("No campsites or hotels within \(Int(StopFinder.radiusMeters / 1000)) km.")
        case .loaded:
            EmptyView()
        }
        if !model.stops.isEmpty {
            panel {
                let splitAt = model.stops.firstIndex { $0.distance >= model.dayEnd.distance } ?? model.stops.count
                ForEach(Array(model.stops.enumerated()), id: \.offset) { index, placed in
                    if index == splitAt { dayEndRule }
                    row(placed, detail: "\(OBCFormat.stopOffset(meters: placed.offset)) · \(relative(placed))",
                        showsDivider: index != splitAt - 1 && index != model.stops.count - 1)
                }
                if splitAt == model.stops.count { dayEndRule }
            }
        }
    }

    @ViewBuilder
    private var results: some View {
        let places = model.results ?? []
        if places.isEmpty {
            message("No places found.")
        } else {
            panel {
                ForEach(Array(places.enumerated()), id: \.offset) { index, placed in
                    row(placed,
                        detail: "\(OBCFormat.stopOffset(meters: placed.offset)) · km \(OBCFormat.distanceValue(meters: placed.distance))",
                        showsDivider: index != places.count - 1)
                }
            }
        }
    }

    /// Where the day ends now, between the stops before it and after it.
    private var dayEndRule: some View {
        HStack(spacing: 8) {
            Text("DAY END NOW · KM \(OBCFormat.distanceValue(meters: model.dayEnd.distance))")
                .font(.system(.caption2, weight: .semibold).monospacedDigit())
                .kerning(1)
                .foregroundStyle(OBCTheme.ink)
                .fixedSize()
            OBCTheme.hairlineStrong.frame(height: 1.5)
        }
        .padding(.vertical, 7)
        .padding(.horizontal, 14)
        .background(OBCTheme.surface2)
        .accessibilityIdentifier("stops.dayEnd")
    }

    private func row(_ placed: PlacedStop, detail: String, showsDivider: Bool) -> some View {
        StopRow(stop: placed.stop, detail: detail, isEnabled: model.canPick(placed), showsDivider: showsDivider) {
            model.pick(placed)
            onDone()
            dismiss()
        }
        .accessibilityIdentifier("stops.row")
    }

    private func relative(_ placed: PlacedStop) -> String {
        let delta = placed.distance - model.dayEnd.distance
        return "\(OBCFormat.shortDistance(meters: delta)) \(delta < 0 ? "before" : "after")"
    }

    private func message(_ text: String) -> some View {
        Text(text)
            .font(.system(.subheadline))
            .foregroundStyle(OBCTheme.secondary)
            .padding(.horizontal, 2)
    }

    private func panel(@ViewBuilder _ content: () -> some View) -> some View {
        VStack(spacing: 0, content: content)
            .background(OBCTheme.surface)
            .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
            .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.hairline))
    }
}
