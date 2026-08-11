import SwiftUI
import OBCWeather

/// The support view (WX13): the WX9 job history ring, rendered readably.
///
/// The ring is the only record of what the background job did — the job itself is invisible by
/// design (two short connections, no foreground state). Twenty rows, newest first, each one a
/// finished exchange: when, how it ended, why, how long the radio was actually held, and which
/// dataset generation answered.
///
/// **Coordinate-free, by construction rather than by redaction.** ``WeatherJobHistoryEntry`` has no
/// field a position could ride in, so this screen cannot leak one however it is read, copied or
/// screenshotted. Since #1244 not even a product id survives: the one provenance field is a bake
/// timestamp, which is the same for every rider on the planet.
public struct WeatherDiagnosticsView: View {
    private let entries: [WeatherJobHistoryEntry]
    private let now: Date

    public init(entries: [WeatherJobHistoryEntry], now: Date = Date()) {
        // Newest first: the row a rider or a support thread wants is always the last run.
        self.entries = entries.reversed()
        self.now = now
    }

    public var body: some View {
        ScrollView {
            VStack(spacing: 26) {
                if entries.isEmpty {
                    emptyGroup
                } else {
                    OBCGroupedSection("Recent weather syncs", footer: footer) {
                        ForEach(Array(entries.enumerated()), id: \.offset) { index, entry in
                            row(entry, isLast: index == entries.count - 1)
                        }
                    }
                    radioGroup
                }
            }
            .padding(.horizontal, 20)
            .padding(.top, 18)
            .padding(.bottom, 30)
        }
        .background(OBCTheme.parchment.ignoresSafeArea())
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("weather.diagnostics.screen")
        .navigationTitle("Sync diagnostics")
        #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
        #endif
    }

    private var emptyGroup: some View {
        OBCGroupedSection(
            "Recent weather syncs",
            footer: "Nothing yet. A row appears each time your OBC asks for weather and the app "
                + "answers — successfully or not."
        ) {
            OBCListRow(
                icon: "tray",
                iconColor: OBCTheme.parchment3,
                label: "No weather syncs recorded",
                showsDivider: false
            )
        }
    }

    private func row(_ entry: WeatherJobHistoryEntry, isLast: Bool) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(WeatherCopy.outcomeLabel(entry.outcome).uppercased())
                    .font(.obcMono(size: 10, weight: .bold))
                    .foregroundStyle(outcomeColor(entry.outcome))
                Text(WeatherCopy.stamp(entry.finishedAt))
                    .font(.obcMono(size: 12))
                    .foregroundStyle(OBCTheme.inkFaint)
                Spacer(minLength: 4)
                // `verbatim`: a plain `Text("req \(id)")` is a LocalizedStringKey, and SwiftUI
                // group-separates interpolated numbers by locale — the request id would render as
                // "4.182" on a German phone, which is not the id the device sent.
                Text(verbatim: "req \(entry.requestID)")
                    .font(.obcMono(size: 11))
                    .foregroundStyle(OBCTheme.inkFaint)
            }
            if entry.outcome != .committed {
                Text(WeatherCopy.failureLabel(entry.failureReason))
                    .font(.system(size: 14))
                    .foregroundStyle(OBCTheme.ink)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let explanation = WeatherCopy.failureExplanation(entry) {
                Text(explanation)
                    .font(.system(size: 12.5))
                    .foregroundStyle(OBCTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Text(detailLine(entry))
                .font(.obcMono(size: 11.5))
                .foregroundStyle(OBCTheme.inkFaint)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 12)
        .padding(.horizontal, 16)
        .overlay(alignment: .bottom) {
            if !isLast { OBCTheme.screenLine.frame(height: 1).padding(.leading, 16) }
        }
    }

    /// The machine line: phase, attempts, bytes, both connected times, generation. Everything a
    /// support thread needs and nothing a location could hide in.
    private func detailLine(_ entry: WeatherJobHistoryEntry) -> String {
        var parts = [WeatherCopy.phaseLabel(entry.phaseReached).lowercased()]
        parts.append("attempt \(max(1, entry.attempts + (entry.outcome == .committed ? 1 : 0)))")
        if let bytes = entry.bundleByteCount { parts.append(WeatherCopy.kilobytes(bytes)) }
        if let read = entry.readConnectedMilliseconds {
            parts.append("read \(WeatherCopy.seconds(milliseconds: read))")
        }
        if let upload = entry.uploadConnectedMilliseconds {
            parts.append("send \(WeatherCopy.seconds(milliseconds: upload))")
        }
        if let generation = entry.precipitationGeneration { parts.append(generation) }
        if let reason = entry.noRainMapReason {
            parts.append("no rain map: \(WeatherCopy.noRainMapReasonLabel(reason))")
        }
        return parts.joined(separator: " · ")
    }

    /// Warning ink is reserved for something that went wrong. A superseded or expired job did not:
    /// the request moved on, or time did, usually with the next run delivering moments later — so
    /// both read in the calm ink the rest of the row uses (#1198 review).
    private func outcomeColor(_ outcome: WeatherJobHistoryEntry.Outcome) -> Color {
        switch outcome {
        case .committed: OBCTheme.forest
        case .failed: OBCTheme.warning
        case .superseded: OBCTheme.inkFaint
        case .agedOut: OBCTheme.inkFaint
        }
    }

    /// Radio time against the epic's budget — the reason both connected times are in the ring.
    private var radioGroup: some View {
        OBCGroupedSection(
            "Radio time",
            footer: "Bluetooth is only on for the two short connections: reading the request, and "
                + "sending the weather back. Everything in between happens with the radio idle."
        ) {
            OBCListRow(
                icon: "dot.radiowaves.left.and.right",
                iconColor: OBCTheme.water,
                label: "Median connected",
                value: medianConnected,
                showsDivider: false
            )
        }
    }

    private var medianConnected: String {
        let totals = entries.compactMap { entry -> Int? in
            let read = entry.readConnectedMilliseconds
            let upload = entry.uploadConnectedMilliseconds
            guard read != nil || upload != nil else { return nil }
            return (read ?? 0) + (upload ?? 0)
        }.sorted()
        guard !totals.isEmpty else { return "—" }
        return WeatherCopy.seconds(milliseconds: totals[totals.count / 2])
    }

    private var footer: String {
        "Times are when each exchange finished. The code at the end of a row is the rain map's "
            + "publication time — the same for everyone, never a place. No coordinate is recorded "
            + "here."
    }
}
