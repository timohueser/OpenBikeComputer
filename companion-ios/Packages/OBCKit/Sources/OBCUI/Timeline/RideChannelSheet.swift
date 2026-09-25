import SwiftUI
import OBCDomain

/// One channel of a ride, large: the chart with the shared cursor, its minimum, average and
/// maximum, and its time in zones. Heart rate and power shade their zone bands; elevation
/// colours its line by grade, as the device's climb screen does.
struct RideChannelSheet: View {
    let timeline: RideTimeline
    let channel: RideTimeline.Channel
    @Binding var cursor: Double?

    @State private var plot = RideTimeline.Plot.empty
    @State private var grades: [Double] = []
    @State private var width: CGFloat = 0

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                header
                chart
                if let stats = timeline.stats(channel) {
                    OBCStatStrip(statItems(stats))
                }
                if let seconds = timeline.timeInZones(channel) {
                    VStack(alignment: .leading, spacing: 8) {
                        OBCEyebrow("Time in zones")
                        RideZoneBar(seconds: seconds)
                    }
                    .accessibilityElement(children: .combine)
                } else if channel == .heartRate || channel == .power, let note = RideZonesCard.missingNote(timeline) {
                    RideZonesNote(text: note)
                }
                if channel == .elevation {
                    gradeLegend
                }
            }
            .padding(.horizontal, 20)
            .padding(.top, 24)
            .padding(.bottom, 24)
        }
        .background(OBCTheme.page.ignoresSafeArea())
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .accessibilityIdentifier("channel.sheet")
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(channel.title)
                .font(.system(.title2, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
                .accessibilityAddTraits(.isHeader)
            Spacer(minLength: 12)
            Text(readout)
                .font(.obcStat(.subheadline))
                .foregroundStyle(OBCTheme.secondary)
                .multilineTextAlignment(.trailing)
                .accessibilityIdentifier("channel.readout")
        }
    }

    /// "KM 12.4 · 156 bpm · Z4" at the cursor; the average before the first scrub.
    private var readout: String {
        let value: Double?
        let place: String
        if let cursor, !plot.values.isEmpty {
            value = plot.values[timeline.column(at: cursor, columns: plot.values.count)]
            place = "KM \(OBCFormat.distanceValue(meters: cursor))"
        } else {
            value = timeline.stats(channel)?.mean
            place = "AVG"
        }
        guard let value else { return place }
        let zone = timeline.zone(channel, value: value).map { " · Z\($0 + 1)" } ?? ""
        return "\(place) · \(channel.format(value)) \(channel.unit)\(zone)"
    }

    private var chart: some View {
        VStack(spacing: 6) {
            RideChannelChart(
                timeline: timeline, channel: channel, plot: plot, grades: grades, style: .detail, cursor: cursor
            )
            .frame(height: 200)
            .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
            .modifier(TimelineScrub(length: timeline.length, cursor: $cursor))
            HStack {
                Text("0 km")
                Spacer()
                Text("\(OBCFormat.distanceValue(meters: timeline.length)) km")
            }
            .font(.system(.caption2, weight: .medium).monospacedDigit())
            .foregroundStyle(OBCTheme.secondary)
        }
        .padding(12)
        .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
        .onChange(of: timelineColumns(width: width), initial: true) { _, columns in
            guard width > 0 else { return }
            plot = timeline.plot(channel, columns: columns)
            if channel == .elevation { grades = timeline.grades(columns: columns) }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(channel.title) chart")
        .accessibilityValue(readout)
        .accessibilityIdentifier("channel.chart")
    }

    private func statItems(_ stats: RideTimeline.Stats) -> [OBCStat] {
        var items = [
            OBCStat(value: channel.format(stats.min), unit: channel.unit, key: "Min"),
            OBCStat(value: channel.format(stats.mean), unit: channel.unit, key: "Avg"),
            OBCStat(value: channel.format(stats.max), unit: channel.unit, key: "Max"),
        ]
        if channel == .elevation, let grade = timeline.maxGradePercent {
            items.append(OBCStat(value: "\(max(Int(grade.rounded()), 0))", unit: "%", key: "Max grade"))
        }
        return items
    }

    private var gradeLegend: some View {
        let bands = ["< 3 %", "3–6", "6–9", "9–12", "> 12 %"]
        return ViewThatFits(in: .horizontal) {
            HStack(spacing: 12) { ForEach(bands.indices, id: \.self) { gradeItem($0, bands[$0]) } }
            VStack(alignment: .leading, spacing: 4) { ForEach(bands.indices, id: \.self) { gradeItem($0, bands[$0]) } }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Line colour by grade")
    }

    private func gradeItem(_ band: Int, _ text: String) -> some View {
        HStack(spacing: 4) {
            Capsule().fill(OBCTheme.gradeBands[band]).frame(width: 14, height: 4)
            Text(text)
                .font(.system(.caption2).monospacedDigit())
                .foregroundStyle(OBCTheme.secondary)
        }
    }
}
