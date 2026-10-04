#if os(iOS)
import OBCDomain
import OBCPlanner
import SwiftUI

/// The filter form, full screen from the summary line. The filters apply with the amber action.
struct PlannerRouteFiltersPage: View {
    let finder: PlannerRouteFinder
    let bike: BikeType
    let onClose: () -> Void
    @State private var draft: PlannerRouteFilters
    @State private var count: Int?

    init(finder: PlannerRouteFinder, bike: BikeType, onClose: @escaping () -> Void) {
        self.finder = finder; self.bike = bike; self.onClose = onClose
        _draft = State(initialValue: finder.filters)
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    OBCGroupedSection("Where", footer: "Or tap the map to move the start.") {
                        OBCListRow(icon: "mappin", label: "Start", value: finder.start?.name)
                        OBCListRow(icon: "circle.dashed", label: "Within", showsDivider: false) {
                            Picker("Within", selection: $draft.radiusKm) {
                                ForEach(SignedRoutes.radii, id: \.self) { Text("\(Int($0)) km").tag($0) }
                            }.labelsHidden().tint(OBCTheme.ink)
                        }
                    }
                    section("Shape") {
                        Picker("Shape", selection: $draft.shape) {
                            ForEach([RouteShape.any, .loop, .oneWay], id: \.self) { Text(PlannerRoutesText.shape($0)).tag($0) }
                        }.pickerStyle(.segmented)
                        Text("A loop is a signed loop. One way is a linear route or an official stage.")
                            .font(.footnote).foregroundStyle(OBCTheme.secondary)
                    }
                    OBCGroupedSection("Size") {
                        bounds("Distance", unit: "km", value: $draft.distanceKm)
                        bounds("Climb", unit: "m", value: $draft.climbM, divider: false)
                    }
                    if bike == .mtb {
                        section("Hardest part") {
                            PlannerGradeRange(range: $draft.hardest)
                            let reading = PlannerRoutesText.reading(draft.hardest)
                            (Text(reading.0).fontWeight(.semibold) + Text(" " + reading.1)).font(.footnote).foregroundStyle(OBCTheme.secondary)
                        }
                    }
                }.padding(20)
            }
            .background(OBCTheme.page)
            .safeAreaInset(edge: .bottom) {
                Button(count.map { "Show \(PlannerRoutesText.noun(draft.shape, count: $0))" } ?? "Show routes") {
                    finder.filters = draft; onClose()
                }
                .buttonStyle(.obcPrimary).padding(.horizontal, 20).padding(.vertical, 12).background(OBCTheme.page)
                .accessibilityIdentifier("planner.routes.apply")
            }
            .navigationTitle("Filters").navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", action: onClose) }
                ToolbarItem(placement: .primaryAction) { Button("Reset") { draft = PlannerRouteFilters() } }
            }
        }
        .tint(OBCTheme.tint)
        .task(id: draft) {
            count = nil
            do {
                try await Task.sleep(for: .milliseconds(200))
                count = try await finder.count(draft, bike: bike)
            } catch {}
        }
    }

    private func section(_ title: String, @ViewBuilder content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title.uppercased()).font(.system(.caption, weight: .semibold)).kerning(0.25).foregroundStyle(OBCTheme.secondary)
                .padding(.horizontal, 8)
            VStack(alignment: .leading, spacing: 10) { content() }
                .padding(16).frame(maxWidth: .infinity, alignment: .leading)
                .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
        }
    }

    /// Two number fields; an empty field is no limit.
    private func bounds(_ label: String, unit: String, value: Binding<RouteBounds>, divider: Bool = true) -> some View {
        func field(_ placeholder: String, _ end: WritableKeyPath<RouteBounds, Double?>) -> some View {
            TextField(placeholder, text: Binding(
                get: { value.wrappedValue[keyPath: end].map { $0.formatted(.number.grouping(.never).precision(.fractionLength(0...1))) } ?? "" },
                set: { text in
                    let number = Double(text.replacingOccurrences(of: ",", with: "."))
                    value.wrappedValue[keyPath: end] = number.flatMap { $0.isFinite && $0 >= 0 ? $0 : nil }
                }))
                .keyboardType(.numberPad).multilineTextAlignment(.center).font(.body.monospacedDigit())
                .frame(width: 64, height: 36).background(OBCTheme.fill, in: RoundedRectangle(cornerRadius: OBCTheme.radiusSmall))
                .accessibilityLabel("\(label) \(placeholder), \(unit)")
        }
        return OBCListRow(label: label, showsDivider: divider) {
            HStack(spacing: 6) {
                field("from", \.from)
                Text("–").foregroundStyle(OBCTheme.secondary)
                field("to", \.to)
                Text(unit).font(.subheadline).foregroundStyle(OBCTheme.secondary).frame(width: 24, alignment: .leading)
            }
        }
    }
}

/// A range over the four grades S0–S3. A tap or a drag moves the nearer end to the grade under the finger.
struct PlannerGradeRange: View {
    @Binding var range: ClosedRange<Int>
    /// Which end the current touch moves, fixed when it starts.
    @State private var movesLower: Bool?

    var body: some View {
        GeometryReader { geometry in
            let step = geometry.size.width / 4
            HStack(spacing: 0) {
                ForEach(0..<4, id: \.self) { grade in
                    let on = range.contains(grade)
                    Text("S\(grade)").font(.subheadline.weight(.semibold).monospacedDigit())
                        .foregroundStyle(on ? OBCTheme.onAmber : OBCTheme.secondary)
                        .frame(width: step, height: 40)
                        .background(on ? OBCTheme.amber : OBCTheme.fill)
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
            .contentShape(Rectangle())
            .gesture(DragGesture(minimumDistance: 0).onChanged { value in
                let grade = min(3, max(0, Int(value.location.x / max(1, step))))
                let start = min(3, max(0, Int(value.startLocation.x / max(1, step))))
                if movesLower == nil {
                    if start < range.lowerBound { movesLower = true }
                    else if start > range.upperBound { movesLower = false }
                    else if range.lowerBound == range.upperBound {
                        // A one-grade range grows the way the finger moves.
                        guard grade != start else { return }
                        movesLower = grade < start
                    } else { movesLower = start - range.lowerBound < range.upperBound - start }
                }
                range = movesLower == true ? min(grade, range.upperBound)...range.upperBound : range.lowerBound...max(grade, range.lowerBound)
            }.onEnded { _ in movesLower = nil })
        }
        .frame(height: 40)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Hardest part")
        .accessibilityValue(PlannerRoutesText.reading(range).0)
        .accessibilityAdjustableAction { direction in
            let upper = min(3, max(range.lowerBound, range.upperBound + (direction == .increment ? 1 : -1)))
            range = range.lowerBound...upper
        }
    }
}
#endif
