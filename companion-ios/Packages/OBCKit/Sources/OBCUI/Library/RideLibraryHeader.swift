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
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .overlay(
                        RoundedRectangle(cornerRadius: OBCTheme.radiusSmall)
                            .strokeBorder(OBCTheme.hairlineStrong)
                    )
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
                .foregroundStyle(isOn ? OBCTheme.surface : OBCTheme.secondary)
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(Capsule().fill(isOn ? OBCTheme.ink : OBCTheme.fill))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("library.chip.\(label)")
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

/// The totals of the filtered rides, beside a thumbnail of their lines.
struct RideTotalsCard: View {
    let scope: String
    let totals: RideTotals
    /// Nil while the lines load; the thumbnail shows the bare grid.
    let lines: RideMapLines?
    let onOpenMap: () -> Void

    var body: some View {
        Button(action: onOpenMap) {
            VStack(spacing: 0) {
                HStack(spacing: 0) {
                    MultiTrackPreviewView(stages: thumbnailStages, showsChrome: false)
                        // Always the grid, like the list cards: a basemap with hundreds of
                        // polylines costs too much for a thumbnail.
                        .environment(\.obcIsOnline, false)
                        .frame(width: 120, height: 96)
                        .overlay(alignment: .trailing) {
                            Rectangle().fill(OBCTheme.hairline).frame(width: 1)
                        }
                    VStack(alignment: .leading, spacing: 4) {
                        OBCEyebrow(scope)
                        Text(OBCFormat.distance(meters: totals.distanceMeters))
                            .font(.system(.title2, weight: .bold))
                            .foregroundStyle(OBCTheme.ink)
                        Text(statLine)
                            .font(.system(.caption2).monospacedDigit())
                            .foregroundStyle(OBCTheme.secondary)
                            .lineLimit(1)
                            .minimumScaleFactor(0.8)
                    }
                    .padding(.horizontal, 12)
                    Spacer(minLength: 0)
                }
                HStack(spacing: 4) {
                    Text("All rides on the map")
                    Image(systemName: "chevron.right")
                        .font(.system(.caption2, weight: .bold))
                    Spacer()
                }
                .font(.system(.caption, weight: .semibold).monospacedDigit())
                .foregroundStyle(OBCTheme.tint)
                .padding(.horizontal, 12)
                .padding(.vertical, 9)
                .overlay(alignment: .top) {
                    Rectangle().fill(OBCTheme.hairline).frame(height: 1)
                }
            }
            .background(OBCTheme.surface)
            .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
            .overlay(
                RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.hairline)
            )
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

    /// The coarsest lines that still read at thumbnail size.
    private var thumbnailStages: [MultiTrackPreviewView.Stage] {
        guard let lines else { return [] }
        let extent = RideMapLines.extentMeters(of: lines.lines(metersPerPoint: .infinity))
        return lines.lines(metersPerPoint: extent / 120).flatMap { line in
            line.pieces.map { MultiTrackPreviewView.Stage(coordinates: $0, color: OBCTheme.ride) }
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
