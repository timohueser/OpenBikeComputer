import SwiftUI
import OBCDomain

extension TransferKind {
    public var title: String {
        switch self {
        case .train: "Train"
        case .bus: "Bus"
        case .ferry: "Ferry"
        case .car: "Car"
        }
    }

    public var systemImage: String {
        switch self {
        case .train: "train.side.front.car"
        case .bus: "bus.fill"
        case .ferry: "ferry.fill"
        case .car: "car.fill"
        }
    }
}

/// The trip page's line between two days with a transfer: "Train · 3.6 km", or
/// "Transfer · 3.6 km" without a label. A tap picks the label.
public struct TripTransferRow: View {
    let kind: TransferKind?
    let meters: Double
    let onPick: (TransferKind?) -> Void

    public init(kind: TransferKind?, meters: Double, onPick: @escaping (TransferKind?) -> Void) {
        self.kind = kind
        self.meters = meters
        self.onPick = onPick
    }

    public var body: some View {
        Menu {
            TransferPicker(kind: kind, onPick: onPick)
        } label: {
            HStack(spacing: 6) {
                Text("\(kind?.title ?? "Transfer") · \(OBCFormat.distance(meters: meters))")
                Image(systemName: "chevron.up.chevron.down").font(.system(size: 9, weight: .semibold))
            }
            .font(.obcMono(size: 12))
            .foregroundStyle(OBCTheme.inkFaint)
            .padding(.vertical, 8)
            .padding(.leading, 38)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .overlay(alignment: .bottom) { OBCTheme.screenLine.frame(height: 1).padding(.leading, 38) }
    }
}

/// The label choice both transfer lines open.
private struct TransferPicker: View {
    let kind: TransferKind?
    let onPick: (TransferKind?) -> Void

    var body: some View {
        Picker("Transfer", selection: Binding(get: { kind }, set: onPick)) {
            ForEach(TransferKind.allCases, id: \.self) { kind in
                Label(kind.title, systemImage: kind.systemImage).tag(Optional(kind))
            }
            Text("None").tag(TransferKind?.none)
        }
    }
}

/// The journal's line between two days with a transfer: "Train · Fischerbach → Teningen". A tap
/// picks the label, as on the trip page.
public struct TripJournalTransfer: View {
    let kind: TransferKind?
    let from: String?
    let to: String?
    let onPick: (TransferKind?) -> Void

    public init(kind: TransferKind?, from: String?, to: String?, onPick: @escaping (TransferKind?) -> Void) {
        self.kind = kind
        self.from = from
        self.to = to
        self.onPick = onPick
    }

    public var body: some View {
        Menu {
            TransferPicker(kind: kind, onPick: onPick)
        } label: {
            line.contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private var line: some View {
        let places = from.flatMap { from in to.map { "\(from) → \($0)" } }
        return HStack(spacing: 10) {
            Path { path in
                path.move(to: CGPoint(x: 0, y: 0.5))
                path.addLine(to: CGPoint(x: 28, y: 0.5))
            }
            .stroke(OBCTheme.lineStrong, style: StrokeStyle(lineWidth: 1.5, lineCap: .round, dash: [1.5, 4]))
            .frame(width: 28, height: 1)
            Image(systemName: kind?.systemImage ?? "arrow.right").font(.system(size: 11))
            Text([kind?.title ?? "Transfer", places].compactMap { $0 }.joined(separator: " · "))
                .font(.obcMono(size: 12))
                .lineLimit(1)
                .minimumScaleFactor(0.85)
            Spacer(minLength: 0)
        }
        .foregroundStyle(OBCTheme.inkFaint)
    }
}

/// The trip's totals under its title: "75.0 of 114 km · 1,119 m ↑ · 5:05 h" during the trip,
/// the ridden share as a bar, and the highlights.
public struct TripReviewTotals: View {
    let ridden: RideTotals
    let plannedMeters: Double
    let isDone: Bool
    let highlights: String?

    public init(ridden: RideTotals, plannedMeters: Double, isDone: Bool, highlights: String?) {
        self.ridden = ridden
        self.plannedMeters = plannedMeters
        self.isDone = isDone
        self.highlights = highlights
    }

    public var body: some View {
        let distance = isDone
            ? OBCFormat.distance(meters: ridden.distanceMeters)
            : "\(OBCFormat.distanceValue(meters: ridden.distanceMeters)) of \(OBCFormat.distance(meters: plannedMeters))"
        VStack(alignment: .leading, spacing: 8) {
            Text("\(distance) · \(OBCFormat.climb(meters: ridden.climbMeters)) · \(OBCFormat.movingTime(ridden.movingTime)) h")
                .font(.obcMono(size: 14, weight: .medium))
                .foregroundStyle(OBCTheme.ink)
            if !isDone {
                GeometryReader { geometry in
                    ZStack(alignment: .leading) {
                        Capsule().fill(OBCTheme.parchment3)
                        Capsule().fill(OBCTheme.trackStroke)
                            .frame(width: geometry.size.width * min(1, ridden.distanceMeters / max(plannedMeters, 1)))
                    }
                }
                .frame(height: 5)
                .accessibilityHidden(true)
            }
            if let highlights {
                Text(highlights)
                    .font(.obcMono(size: 12))
                    .foregroundStyle(OBCTheme.inkFaint)
            }
        }
    }
}

/// One ridden day of the journal: the day, its header and note, its photos, and its rides when
/// it has more than one. The day opens its ride; a day with two rides opens each from its row.
public struct TripJournalDayEntry: View {
    let number: Int
    let title: String?
    let header: String
    let note: String
    let photos: [RidePhoto]
    let thumbnails: [String: Data]
    let rides: [RideSummary]
    let onOpenRide: (RideID) -> Void

    /// Photos past this count show as "+N" on the last tile.
    static let tileCount = 4

    public init(
        number: Int, title: String?, header: String, note: String, photos: [RidePhoto], thumbnails: [String: Data],
        rides: [RideSummary], onOpenRide: @escaping (RideID) -> Void
    ) {
        self.number = number
        self.title = title
        self.header = header
        self.note = note
        self.photos = photos
        self.thumbnails = thumbnails
        self.rides = rides
        self.onOpenRide = onOpenRide
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if rides.count == 1 {
                Button { onOpenRide(rides[0].id) } label: { day.contentShape(Rectangle()) }
                    .buttonStyle(.plain)
            } else {
                day
            }
            if rides.count > 1 {
                OBCGroupedSection {
                    ForEach(Array(rides.enumerated()), id: \.element.id) { index, ride in
                        OBCListRow(
                            label: ride.name, detail: OBCFormat.rideStatsLine(ride), showsChevron: true,
                            showsDivider: index < rides.count - 1
                        ) { onOpenRide(ride.id) }
                    }
                }
            }
        }
    }

    private var day: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("Day \(number)").font(.obcSerif(size: 22)).foregroundStyle(OBCTheme.ink)
                if let title {
                    Text(title).font(.obcSerif(size: 22, weight: .regular)).foregroundStyle(OBCTheme.inkSoft)
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
                if rides.count == 1 {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(OBCTheme.inkFaint)
                }
            }
            if note.isEmpty {
                OBCEyebrow(header)
            } else {
                DayNoteText(header: header, note: note)
            }
            if !photos.isEmpty { photoTiles }
        }
    }

    private var photoTiles: some View {
        HStack(spacing: 6) {
            ForEach(Array(photos.prefix(Self.tileCount).enumerated()), id: \.element.id) { index, photo in
                PhotoThumbnail(data: thumbnails[photo.assetID])
                    .overlay {
                        let more = photos.count - Self.tileCount
                        if index == Self.tileCount - 1, more > 0 {
                            OBCTheme.ink.opacity(0.45)
                            Text("+\(more + 1)").font(.obcMono(size: 13, weight: .semibold)).foregroundStyle(.white)
                        }
                    }
                    .frame(width: 76, height: 76)
                    .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusSmall))
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(photos.count) photos")
    }
}
