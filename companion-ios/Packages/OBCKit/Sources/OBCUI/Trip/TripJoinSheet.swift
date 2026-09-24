import SwiftUI
import OBCDomain
import OBCTransport

/// "Make a trip from these 4 routes?": several files that arrived together, one day per file in
/// the proposed order. The rider drags the rows to change the order; the line above a row says
/// whether it joins the day before or leaves a gap. Not now imports them as routes.
public struct TripJoinSheet: View {
    /// One file as the sheet lists it.
    public struct File: Identifiable, Sendable {
        public let id: Int
        public let fileName: String
        public let points: [RoutePoint]
        let distanceMeters: Double
        let climbMeters: Double

        public init(id: Int, fileName: String, points: [RoutePoint]) {
            self.id = id
            self.fileName = fileName
            self.points = points
            let totals = RouteObjectCodec.totals(points: points)
            distanceMeters = Double(totals?.distanceMeters ?? 0)
            climbMeters = Double(totals?.ascentMeters ?? 0)
        }

        var joinFile: TripJoin.File {
            TripJoin.File(
                name: fileName, start: points.first?.coordinate ?? Coordinate(latitude: 0, longitude: 0),
                end: points.last?.coordinate ?? Coordinate(latitude: 0, longitude: 0))
        }
    }

    @State private var order: [File]
    private let onMakeTrip: ([File]) -> Void
    private let onNotNow: () -> Void

    public init(files: [File], onMakeTrip: @escaping ([File]) -> Void, onNotNow: @escaping () -> Void) {
        let proposed = TripJoin.proposedOrder(files.map(\.joinFile)).map { files[$0] }
        _order = State(initialValue: proposed)
        self.onMakeTrip = onMakeTrip
        self.onNotNow = onNotNow
    }

    public var body: some View {
        NavigationStack {
            List {
                header
                    .listRowSeparator(.hidden)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 14, trailing: 20))
                ForEach(Array(order.enumerated()), id: \.element.id) { index, file in
                    row(index: index, file: file)
                        .listRowBackground(OBCTheme.surface)
                        .listRowInsets(EdgeInsets(top: 0, leading: 16, bottom: 0, trailing: 16))
                        .accessibilityIdentifier("join.day.\(index)")
                }
                .onMove { order.move(fromOffsets: $0, toOffset: $1) }
                Text(OBCFormat.tripSubtitle(
                    dayCount: order.count,
                    distanceMeters: order.reduce(0) { $0 + $1.distanceMeters },
                    elevationGainMeters: order.reduce(0) { $0 + $1.climbMeters }))
                    .font(.system(.caption).monospacedDigit())
                    .foregroundStyle(OBCTheme.secondary)
                    .listRowSeparator(.hidden)
                    .listRowBackground(Color.clear)
                    .accessibilityIdentifier("join.totals")
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .background(OBCTheme.page.ignoresSafeArea())
            #if os(iOS)
            .environment(\.editMode, .constant(.active))
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Not now", action: onNotNow)
                        .accessibilityIdentifier("join.notNow")
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Make trip") { onMakeTrip(order) }
                        .fontWeight(.semibold)
                        .accessibilityIdentifier("join.makeTrip")
                }
            }
        }
        .tint(OBCTheme.tint)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Make a trip from these \(order.count) routes?")
                .font(.system(.title, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
                .accessibilityIdentifier("join.title")
            MultiTrackPreviewView(stages: order.enumerated().map { index, file in
                MultiTrackPreviewView.Stage(
                    coordinates: file.points.map(\.coordinate), color: OBCTheme.stageColor(index: index))
            })
            .frame(height: 150)
        }
    }

    private func row(index: Int, file: File) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            if index > 0 { joinLine(from: order[index - 1], to: file) }
            HStack(spacing: 12) {
                Circle().fill(OBCTheme.stageColor(index: index)).frame(width: 10, height: 10)
                VStack(alignment: .leading, spacing: 3) {
                    Text("Day \(index + 1)")
                        .font(.system(.callout))
                        .foregroundStyle(OBCTheme.ink)
                    Text("\(file.fileName) · \(OBCFormat.distance(meters: file.distanceMeters)) · \(OBCFormat.climb(meters: file.climbMeters))")
                        .font(.system(.caption).monospacedDigit())
                        .foregroundStyle(OBCTheme.secondary)
                        .lineLimit(2)
                }
            }
            .padding(.vertical, 12)
        }
    }

    /// "joins" when the day before ends within 200 m of this start, else "gap 3.4 km".
    private func joinLine(from previous: File, to file: File) -> some View {
        let gap = TripJoin.gaps([previous.joinFile, file.joinFile])[0]
        let joins = gap <= TripJoin.joinMeters
        return Text(joins ? "joins" : "gap \(OBCFormat.distance(meters: gap))")
            .font(.system(.caption).monospacedDigit())
            .foregroundStyle(joins ? OBCTheme.secondary : OBCTheme.danger)
            .padding(.top, 8)
            .padding(.leading, 22)
    }
}
