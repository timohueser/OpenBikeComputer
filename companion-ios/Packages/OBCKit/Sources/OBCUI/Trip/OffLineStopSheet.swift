import SwiftUI
import OBCDomain

/// "Camp Ulrichen · 400 m off the line": how the day reaches a stop off the line. Two options,
/// each with a sketch and what it adds; a pick closes the sheet. When neither routes, one plain
/// line says why and one action ends the day on the line.
public struct OffLineStopSheet: View {
    let model: OffLineStopModel
    /// The colour of the day that ends at the stop.
    let color: Color

    @Environment(\.dismiss) private var dismiss

    public init(model: OffLineStopModel, color: Color) {
        self.model = model
        self.color = color
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            header
            if let failure = model.failure {
                Text(Self.message(failure))
                    .font(.system(.subheadline))
                    .foregroundStyle(OBCTheme.ink)
                    .accessibilityIdentifier("offLine.failure")
                Button("End the day on the line") {
                    model.endOnLine()
                    dismiss()
                }
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("offLine.onLine")
            } else {
                HStack(spacing: 12) {
                    card(.outAndBack, title: "Out and back", note: "main line unchanged", option: model.outAndBack)
                    card(.via, title: "Via the stop", note: "dashed = old line", option: model.via)
                }
                if model.isDownloading, model.outAndBack == .routing || model.via == .routing {
                    Text("Getting map data…")
                        .font(.system(.caption).monospacedDigit())
                        .foregroundStyle(OBCTheme.secondary)
                        .accessibilityIdentifier("offLine.downloading")
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 20)
        .padding(.top, 24)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(OBCTheme.page.ignoresSafeArea())
        .task { await model.load() }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("offLine.sheet")
    }

    private var header: some View {
        HStack(spacing: 12) {
            StopIcon(kind: model.stop.kind)
            VStack(alignment: .leading, spacing: 2) {
                Text(model.stop.name)
                    .font(.system(.title2, weight: .bold))
                    .foregroundStyle(OBCTheme.ink)
                    .lineLimit(1)
                Text("\(OBCFormat.stopOffset(meters: model.offset)) · end of Day \(model.day + 1)")
                    .font(.system(.caption).monospacedDigit())
                    .foregroundStyle(OBCTheme.secondary)
            }
        }
    }

    private func card(_ mode: OffLineStopModel.Mode, title: String, note: String, option: OffLineStopModel.Option) -> some View {
        let isCurrent = model.current == mode
        let isReady = if case .ready = option { true } else { false }
        return Button {
            model.pick(mode)
            dismiss()
        } label: {
            VStack(alignment: .leading, spacing: 8) {
                StopRouteSketch(mode: mode, color: color)
                    .frame(height: 44)
                Text(title)
                    .font(.system(.callout, weight: .semibold))
                    .foregroundStyle(OBCTheme.ink)
                Group {
                    switch option {
                    case .routing:
                        ProgressView().controlSize(.small).tint(OBCTheme.secondary)
                    case .ready(_, let extra):
                        Text("\(OBCFormat.extraDistance(meters: extra)) · \(note)")
                    case .failed:
                        Text("No road found")
                    case .noRoom:
                        Text("No room for a via here")
                    }
                }
                .font(.system(.caption).monospacedDigit())
                .foregroundStyle(OBCTheme.secondary)
                .frame(minHeight: 32, alignment: .topLeading)
                .fixedSize(horizontal: false, vertical: true)
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(OBCTheme.surface)
            .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
            .overlay(
                RoundedRectangle(cornerRadius: OBCTheme.radiusPanel)
                    .strokeBorder(isCurrent ? OBCTheme.ink : OBCTheme.hairline, lineWidth: isCurrent ? 1.5 : 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!isReady)
        .accessibilityIdentifier(mode == .outAndBack ? "offLine.outAndBack" : "offLine.via")
    }

    static func message(_ failure: LegRouteFailure) -> String {
        switch failure {
        case .noRoad: "No road to this stop found."
        case .noConnection: "No connection. The router needs a connection once for this area."
        case .mapData: "The map data for this area did not load."
        case .noMap: "No map data for this area."
        }
    }
}

/// The plan's sketch of a mode: the line in the day's colour and the stop as a ring above it.
/// An out and back is a spur up to the stop; a via leaves the line, runs through the stop and
/// rejoins it, with the old line dashed below.
struct StopRouteSketch: View {
    let mode: OffLineStopModel.Mode
    let color: Color

    var body: some View {
        Canvas { context, size in
            let w = size.width, base = size.height - 6, top = CGFloat(10)
            let stroke = StrokeStyle(lineWidth: 3, lineCap: .round, lineJoin: .round)
            var line = Path()
            switch mode {
            case .outAndBack:
                line.move(to: CGPoint(x: 4, y: base))
                line.addLine(to: CGPoint(x: w - 4, y: base))
                line.move(to: CGPoint(x: w / 2, y: base))
                line.addLine(to: CGPoint(x: w / 2, y: top + 5))
            case .via:
                var old = Path()
                old.move(to: CGPoint(x: w * 0.3, y: base))
                old.addLine(to: CGPoint(x: w * 0.7, y: base))
                context.stroke(old, with: .color(OBCTheme.secondary), style: StrokeStyle(lineWidth: 2, dash: [4, 3]))
                line.move(to: CGPoint(x: 4, y: base))
                line.addLine(to: CGPoint(x: w * 0.3, y: base))
                line.addLine(to: CGPoint(x: w / 2, y: top + 5))
                line.addLine(to: CGPoint(x: w * 0.7, y: base))
                line.addLine(to: CGPoint(x: w - 4, y: base))
            }
            context.stroke(line, with: .color(color), style: stroke)
            let ring = Path(ellipseIn: CGRect(x: w / 2 - 5, y: top - 5, width: 10, height: 10))
            context.fill(ring, with: .color(OBCTheme.surface))
            context.stroke(ring, with: .color(OBCTheme.ink), lineWidth: 1.5)
        }
        .accessibilityHidden(true)
    }
}
