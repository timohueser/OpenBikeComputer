#if os(iOS)
import SwiftUI
import OBCDomain

struct PlannerPreviewProfile: View {
    let profile: PlannerPreviewGrade
    let height: CGFloat
    let visibleRange: ClosedRange<Double>?
    @Binding var selectedFraction: Double?

    var body: some View {
        let profile = profile
        // With no route in view the profile shows the whole route, so the drawer never
        // swaps content or changes height while the map pans.
        let range = visibleRange.flatMap { $0.upperBound > $0.lowerBound ? $0 : nil } ?? 0...1
        VStack(spacing: 4) {
            Text(readout(profile, range: range))
                .font(.system(.caption, weight: .semibold).monospacedDigit())
                .foregroundStyle(OBCTheme.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
            plot(profile, range: range)
                .background(OBCTheme.surface)
                .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
            HStack {
                Text("\(range.lowerBound * profile.distance / 1_000, specifier: "%.1f") km")
                Spacer()
                Text("\(range.upperBound * profile.distance / 1_000, specifier: "%.1f") km")
            }
            .font(.caption2.monospacedDigit()).foregroundStyle(OBCTheme.secondary)
        }
    }

    private func plot(_ profile: PlannerPreviewGrade, range: ClosedRange<Double>) -> some View {
        let segments = profile.segments(in: range)
        let low = (segments.map { min($0.start, $0.end) }.min() ?? 0) - 20
        let high = max(low + 80, (segments.map { max($0.start, $0.end) }.max() ?? 0) + 20)
        let span = range.upperBound - range.lowerBound
        return GeometryReader { geometry in
            Canvas { context, size in
                func point(_ fraction: Double, _ elevation: Double) -> CGPoint {
                    CGPoint(x: (fraction - range.lowerBound) / span * size.width,
                            y: (1 - (elevation - low) / (high - low)) * size.height)
                }
                // One fill under the whole curve, as the app's other profiles draw it; the grade
                // lives in the stroke only.
                if let first = segments.first, let last = segments.last {
                    let area = Path { path in
                        path.move(to: CGPoint(x: point(first.from, low).x, y: size.height))
                        path.addLine(to: point(first.from, first.start))
                        for segment in segments { path.addLine(to: point(segment.to, segment.end)) }
                        path.addLine(to: CGPoint(x: point(last.to, low).x, y: size.height))
                        path.closeSubpath()
                    }
                    context.fill(area, with: .color(OBCTheme.profileFill))
                }
                for segment in segments {
                    let a = point(segment.from, segment.start), b = point(segment.to, segment.end)
                    context.stroke(Path { $0.move(to: a); $0.addLine(to: b) },
                                   with: .color(color(PlannerPreviewGrade.band(segment.grade))),
                                   style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                }
                if let fraction = selectedFraction, range.contains(fraction) {
                    let x = point(fraction, low).x
                    context.stroke(Path { $0.move(to: CGPoint(x: x, y: 0)); $0.addLine(to: CGPoint(x: x, y: size.height)) },
                                   with: .color(OBCTheme.ink), lineWidth: 1)
                    if let elevation = profile.reading(at: fraction).elevation {
                        let center = point(fraction, elevation)
                        context.fill(Path(ellipseIn: CGRect(x: center.x - 3, y: center.y - 3, width: 6, height: 6)),
                                     with: .color(OBCTheme.ink))
                    }
                }
            }
            .overlay { if segments.isEmpty { Text("Elevation unavailable").font(.caption).foregroundStyle(OBCTheme.secondary) } }
            .contentShape(Rectangle())
            .gesture(DragGesture(minimumDistance: 0).onChanged { value in
                selectedFraction = range.lowerBound + min(1, max(0, value.location.x / max(1, geometry.size.width))) * span
            })
        }
        .frame(height: max(44, height))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Elevation profile in map view")
        .accessibilityValue("\(readout(profile, range: range)). Grade uses a 100 meter average.")
        .accessibilityAdjustableAction { direction in
            let current = selectedFraction.map { min(range.upperBound, max(range.lowerBound, $0)) } ?? range.lowerBound
            selectedFraction = min(range.upperBound, max(range.lowerBound, current + (direction == .increment ? 0.05 : -0.05) * span))
        }
        .accessibilityIdentifier("planner.elevation")
    }

    private func color(_ band: Int) -> Color {
        let value = PlannerPreviewGrade.palette[band]
        return Color(day: value.light, tent: value.dark)
    }

    private func readout(_ profile: PlannerPreviewGrade, range: ClosedRange<Double>) -> String {
        guard let fraction = selectedFraction, range.contains(fraction) else { return "Elevation" }
        let reading = profile.reading(at: fraction)
        let elevation = reading.elevation.map { "\(Int($0.rounded())) m" } ?? "Elevation unknown"
        return String(format: "%.1f km · %@ · %@", profile.distance * fraction / 1_000, elevation, PlannerPreviewGrade.label(reading.grade))
    }
}
#endif
