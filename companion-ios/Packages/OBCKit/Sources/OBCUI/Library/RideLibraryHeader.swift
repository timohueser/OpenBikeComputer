import SwiftUI
import OBCDomain

/// The head of the Tracked list: the year and bike-type filter, then the totals card, which
/// opens the all-rides map.
struct RideLibraryHeader: View {
    let model: RideLibraryModel
    let onOpenMap: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            RideFilterBar(model: model)
            RideTotalsCard(
                scope: model.scopeLabel,
                totals: model.totals,
                lines: model.filteredMapLines,
                onOpenMap: onOpenMap
            )
        }
    }
}

/// The year menu and the bike-type chips. The list, the totals and the map share this filter.
struct RideFilterBar: View {
    let model: RideLibraryModel

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                Menu {
                    ForEach(model.years, id: \.self) { year in
                        Button(String(year)) { model.selectYear(year) }
                    }
                    Button("All years") { model.selectYear(nil) }
                } label: {
                    HStack(spacing: 4) {
                        Text(model.year.map(String.init) ?? "All years")
                        Image(systemName: "chevron.down")
                            .font(.system(.caption2, weight: .bold))
                    }
                    .font(.system(.footnote, weight: .semibold).monospacedDigit())
                    .foregroundStyle(OBCTheme.ink)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 7)
                    .background(OBCTheme.surface, in: Capsule())
                    .frame(minHeight: 44)
                    .contentShape(Rectangle())
                }
                .accessibilityIdentifier("library.year")

                chip("All", isOn: model.bikeType == nil) { model.selectBikeType(nil) }
                ForEach(model.bikeTypes, id: \.self) { type in
                    chip(type.name, isOn: model.bikeType == type) { model.selectBikeType(type) }
                }
            }
        }
        .scrollClipDisabled()
    }

    private func chip(_ label: String, isOn: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(.system(.footnote, weight: .semibold))
                .foregroundStyle(isOn ? OBCTheme.page : OBCTheme.secondary)
                .padding(.horizontal, 12)
                .padding(.vertical, 7)
                .background(Capsule().fill(isOn ? OBCTheme.ink : OBCTheme.fill))
                .frame(minHeight: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("library.chip.\(label)")
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

/// The totals of the filtered rides, beside a sketch of their lines.
struct RideTotalsCard: View {
    let scope: String
    let totals: RideTotals
    /// Nil while the lines load; the sketch shows the bare ground.
    let lines: RideMapLines?
    let onOpenMap: () -> Void

    var body: some View {
        Button(action: onOpenMap) {
            VStack(spacing: 0) {
                HStack(spacing: 14) {
                    TrackPreviewView(tracks: sketchTracks, showsEnds: false, showsChrome: false)
                        .frame(width: 96, height: 72)
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                    VStack(alignment: .leading, spacing: 3) {
                        OBCEyebrow(scope)
                        Text(OBCFormat.distance(meters: totals.distanceMeters))
                            .font(.obcStat(.title2))
                            .foregroundStyle(OBCTheme.ink)
                        Text(statLine)
                            .font(.system(.subheadline).monospacedDigit())
                            .foregroundStyle(OBCTheme.secondary)
                    }
                    Spacer(minLength: 0)
                }
                .padding(12)
                HStack(spacing: 4) {
                    Text("All rides on the map")
                    Image(systemName: "chevron.right")
                        .font(.system(.caption2, weight: .bold))
                    Spacer()
                }
                .font(.system(.subheadline, weight: .semibold))
                .foregroundStyle(OBCTheme.tint)
                .padding(.horizontal, 12)
                .frame(minHeight: 44)
                .overlay(alignment: .top) {
                    OBCTheme.hairline.frame(height: 1).padding(.leading, 12)
                }
            }
            .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("library.totals")
    }

    private var statLine: String {
        [
            totals.rideCount == 1 ? "1 ride" : "\(totals.rideCount) rides",
            OBCFormat.movingTime(totals.movingTime),
            OBCFormat.climb(meters: totals.climbMeters),
        ].joined(separator: " · ")
    }

    /// The coarsest lines that still read at sketch size.
    private var sketchTracks: [(coordinates: [Coordinate], ink: TrackPreviewView.Ink)] {
        guard let lines else { return [] }
        let extent = RideMapLines.extentMeters(of: lines.lines(metersPerPoint: .infinity))
        return lines.lines(metersPerPoint: extent / 96).flatMap { line in
            line.pieces.map { (coordinates: $0, ink: TrackPreviewView.Ink.ride) }
        }
    }
}

extension RideLibraryModel {
    /// "2026 · All types", "All years · Road".
    var scopeLabel: String {
        "\(year.map(String.init) ?? "All years") · \(bikeType?.name ?? "All types")"
    }
}

extension RideMapLines {
    /// The larger side of the lines' bounding box, in metres.
    static func extentMeters(of lines: [RideMapLine]) -> Double {
        let coordinates = lines.flatMap { $0.pieces.joined() }
        guard let first = coordinates.first else { return 0 }
        var (south, north, west, east) = (first.latitude, first.latitude, first.longitude, first.longitude)
        for c in coordinates {
            south = min(south, c.latitude); north = max(north, c.latitude)
            west = min(west, c.longitude); east = max(east, c.longitude)
        }
        let width = (east - west) * 111_320 * cos((south + north) / 2 * .pi / 180)
        return max(width, (north - south) * 111_320)
    }
}
