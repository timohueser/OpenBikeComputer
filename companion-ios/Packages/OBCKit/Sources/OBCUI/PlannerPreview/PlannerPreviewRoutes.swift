import Foundation
import OBCDomain
import OBCPlanner
#if os(iOS)
import SwiftUI
#endif

/// The words of the Routes view. Hiking grades are T1–T4, mountain bike grades S0–S3.
@MainActor enum PlannerRoutesText {
    static let levels = ["", "Local", "Regional", "National", "International"]
    static func grade(_ index: Int, mtb: Bool) -> String { mtb ? "S\(index)" : "T\(index + 1)" }
    static func graded(_ activity: RouteActivity) -> Bool { activity == .hiking || activity == .mtb }
    /// The word in "No signed … loops".
    static func activity(_ activity: RouteActivity) -> String {
        switch activity { case .hiking: "hiking"; case .mtb: "mountain bike"; case .road, .gravel, .touring: "cycling" }
    }
    static func noun(_ shape: RouteShape, count: Int) -> String {
        "\(count) \(shape == .loop ? "loop" : "route")\(count == 1 ? "" : "s")"
    }
    static func shape(_ shape: RouteShape) -> String {
        switch shape { case .any: "Any"; case .loop: "Loop"; case .oneWay: "One way" }
    }
    static func range(_ bounds: RouteBounds, unit: String) -> String? {
        let number = { (value: Double) in value.formatted(.number.precision(.fractionLength(0...1))) }
        switch (bounds.from, bounds.to) {
        case let (from?, to?): return "\(number(from))–\(number(to)) \(unit)"
        case let (from?, nil): return "\(number(from)) \(unit) or more"
        case let (nil, to?): return "up to \(number(to)) \(unit)"
        default: return nil
        }
    }
    static func hardest(_ range: ClosedRange<Int>, mtb: Bool) -> String {
        let grade = { self.grade($0, mtb: mtb) }
        return range.lowerBound == 0 ? "up to \(grade(range.upperBound))" : range.lowerBound == range.upperBound ? grade(range.lowerBound)
            : "\(grade(range.lowerBound)) or harder"
    }
    /// The reading under the hardest-part control, and its detail.
    static func reading(_ range: ClosedRange<Int>, mtb: Bool) -> (String, String) {
        let grade = { self.grade($0, mtb: mtb) }
        let (low, high) = (grade(range.lowerBound), grade(range.upperBound))
        let names = range.map(grade)
        let list = names.count > 1 ? names.dropLast().joined(separator: ", ") + " or " + names.last! : names[0]
        let reading = range.lowerBound == 0 ? "Up to \(high)." : range.upperBound == 3 ? "Must include \(low) or harder."
            : low == high ? "Hardest part \(low)." : "Must include \(low) or harder, up to \(high)."
        return (reading, range.lowerBound == 0 ? "Routes whose hardest part is \(list). A \(mtb ? "trail" : "path") without a grade counts as \(grade(0))."
                : "Only routes with a part graded \(list).")
    }
    static func summary(_ filters: PlannerRouteFilters, activity: RouteActivity) -> String {
        ["\(Int(filters.radiusKm)) km", shape(filters.shape), range(filters.distanceKm, unit: "km"), range(filters.climbM, unit: "m"),
         graded(activity) ? hardest(filters.hardest, mtb: activity == .mtb) : nil].compactMap { $0 }.joined(separator: " · ")
    }
    static func kind(_ route: CatalogRecord, finder: PlannerRouteFinder) -> String {
        if let stages = route.stages { return "\(stages.count) stages" }
        if let stage = route.stage {
            let total = route.parent.flatMap { finder.record($0)?.stages?.count }
            return total.map { "Stage \(stage) of \($0)" } ?? "Stage \(stage)"
        }
        return route.loop ? "Loop" : "One way"
    }
    static func network(_ route: CatalogRecord) -> String {
        let word = switch route.kind { case .hiking, .foot: "hiking route"; case .bicycle: "cycling route"; case .mtb: "mountain bike route" }
        let level = levels[min(route.rank, 4)]
        return level.isEmpty ? word.prefix(1).uppercased() + word.dropFirst() : "\(level) \(word)"
    }
    static func website(_ text: String?) -> URL? {
        guard let text = text?.trimmingCharacters(in: .whitespaces), !text.isEmpty else { return nil }
        return URL(string: text.contains("://") ? text : "https://" + text).flatMap { ["http", "https"].contains($0.scheme) ? $0 : nil }
    }
}

#if os(iOS)
/// A route's number in the list and on the map, in its network colour.
struct PlannerRouteNumber: View {
    let number: Int
    let rank: Int
    var selected = false
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let traits = UITraitCollection(userInterfaceStyle: scheme == .dark ? .dark : .light)
        let color = Color(uiColor: PlannerPreviewNetworkStyle.color(rank: rank, traits: traits).withAlphaComponent(1))
        Text("\(number)").font(.system(.caption, weight: .semibold).monospacedDigit())
            .foregroundStyle(selected ? OBCTheme.surface : OBCTheme.ink)
            .frame(minWidth: 26, minHeight: 26)
            .background(selected ? OBCTheme.route : OBCTheme.surface, in: Circle())
            .overlay(Circle().strokeBorder(selected ? OBCTheme.route : color, lineWidth: 2))
            .accessibilityHidden(true)
    }
}

/// The `ref` as a plain badge where the route has no trail symbol, as on French GR and PR routes.
struct PlannerRouteRefBadge: View {
    let route: CatalogRecord
    var body: some View {
        if route.symbol == nil, let ref = route.ref {
            Text(ref).font(.system(.caption2, weight: .bold).monospacedDigit()).foregroundStyle(OBCTheme.secondary)
                .lineLimit(1).padding(.horizontal, 4).frame(minWidth: 24, minHeight: 20)
                .overlay(RoundedRectangle(cornerRadius: 3).strokeBorder(OBCTheme.secondary, lineWidth: 1.5))
                .accessibilityLabel("Route number \(ref)")
        }
    }
}

/// The list in the middle sheet: the way back to the plan, the summary with Filters, the count, and one list.
struct PlannerRoutesList: View {
    let finder: PlannerRouteFinder
    let activity: RouteActivity
    /// The plan under the view, for the line that leads back to it.
    let plan: (title: String, meters: Double)?
    let onShowPlan: () -> Void
    let onFilters: () -> Void
    let onSelect: (CatalogRecord) -> Void
    let onRetry: () -> Void

    private var graded: Bool { PlannerRoutesText.graded(activity) }
    private var place: String { finder.start?.name ?? "the start" }

    var body: some View {
        LazyVStack(alignment: .leading, spacing: 10) {
            if let plan {
                HStack(spacing: 4) {
                    Text("Your plan: \(plan.title), \(OBCFormat.distance(meters: plan.meters))").lineLimit(1)
                    Text("·")
                    Button("Show", action: onShowPlan).fontWeight(.semibold).accessibilityIdentifier("planner.routes.showPlan")
                }.font(.subheadline).foregroundStyle(OBCTheme.secondary).frame(minHeight: 32)
            }
            Button(action: onFilters) {
                HStack {
                    Text(PlannerRoutesText.summary(finder.filters, activity: activity)).font(.subheadline).foregroundStyle(OBCTheme.ink)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Label("Filters", systemImage: "slider.horizontal.3").font(.subheadline.weight(.semibold)).foregroundStyle(OBCTheme.tint)
                }
                .padding(.horizontal, 12).frame(minHeight: 44)
                .background(OBCTheme.fill, in: RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
                .contentShape(Rectangle())
            }.buttonStyle(.plain).accessibilityIdentifier("planner.routes.filters")
            results
        }
    }

    @ViewBuilder private var results: some View {
        let filters = finder.filters
        if finder.status == .failed {
            note("The routes could not load. Check your connection and try again.")
            Button("Try again", action: onRetry).frame(minHeight: 44)
        } else if finder.status == .loading && finder.progress.loaded < finder.progress.total {
            note("Loading the routes around \(place) · \(finder.progress.loaded) of \(finder.progress.total) map cells")
        } else if finder.status == .ready && finder.matches.isEmpty {
            empty(filters)
        } else if !finder.matches.isEmpty {
            HStack {
                Text(PlannerRoutesText.noun(filters.shape, count: finder.matches.count)).font(.subheadline.weight(.semibold))
                Text("·").foregroundStyle(OBCTheme.secondary)
                Menu {
                    Picker("Sort", selection: Binding(get: { finder.filters.sort }, set: { finder.filters.sort = $0 })) {
                        ForEach(Self.sorts.indices, id: \.self) { Text(Self.sorts[$0].1).tag(Self.sorts[$0].0) }
                    }
                } label: {
                    HStack(spacing: 3) {
                        Text(Self.sorts.first { $0.0 == filters.sort }?.1.lowercased() ?? "")
                        Image(systemName: "chevron.down").font(.caption2.weight(.semibold))
                    }.font(.subheadline).foregroundStyle(OBCTheme.tint).frame(minHeight: 32)
                }.accessibilityLabel("Sort").accessibilityIdentifier("planner.routes.sort")
            }
            if finder.offline { note("Only routes inside your download are shown.") }
            VStack(spacing: 0) {
                ForEach(Array(finder.matches.prefix(finder.shown).enumerated()), id: \.element.route.id) { index, match in
                    Button { onSelect(match.route) } label: { row(match, number: index + 1) }
                        .buttonStyle(.plain).accessibilityIdentifier("planner.routes.row.\(match.route.id)")
                    Divider().overlay(OBCTheme.hairline)
                }
            }
            if finder.matches.count > finder.shown {
                Button("Show 20 more") { finder.shown += 20 }.frame(minHeight: 44)
            }
            Text("© OpenStreetMap contributors").font(.caption2).foregroundStyle(OBCTheme.secondary)
        }
    }

    static let sorts: [(RouteSort, String)] = [(.nearest, "Nearest first"), (.shortest, "Shortest first"), (.longest, "Longest first"),
                                               (.mostClimb, "Most climb first"), (.leastClimb, "Least climb first")]

    @ViewBuilder private func empty(_ filters: PlannerRouteFilters) -> some View {
        switch finder.hint {
        case .filter(let filter):
            let name = switch filter { case .distance: "distance"; case .climb: "climb"; case .hardest: "hardest part" }
            let value = switch filter {
            case .distance: PlannerRoutesText.range(filters.distanceKm, unit: "km") ?? ""
            case .climb: PlannerRoutesText.range(filters.climbM, unit: "m") ?? ""
            case .hardest: PlannerRoutesText.hardest(filters.hardest, mtb: activity == .mtb)
            }
            note("No routes match the \(name) \(value).")
            Button("Clear \(name)") {
                switch filter {
                case .distance: finder.filters.distanceKm = .init()
                case .climb: finder.filters.climbM = .init()
                case .hardest: finder.filters.hardest = 0...3
                }
            }.buttonStyle(.obcGhost)
        case .wider(let radius, let count):
            note("No signed \(PlannerRoutesText.activity(activity)) \(filters.shape == .loop ? "loops" : "routes") within \(Int(filters.radiusKm)) km of \(place). \(count) within \(Int(radius)) km.")
            Button("Search within \(Int(radius)) km") { finder.filters.radiusKm = radius }.buttonStyle(.obcGhost)
        case nil:
            note("No signed \(PlannerRoutesText.activity(activity)) \(filters.shape == .loop ? "loops" : "routes") within \(Int(filters.radiusKm)) km of \(place).")
        }
    }

    private func note(_ text: String) -> some View {
        Text(text).font(.subheadline).foregroundStyle(OBCTheme.secondary).fixedSize(horizontal: false, vertical: true)
    }

    private func row(_ match: RouteMatch, number: Int) -> some View {
        let route = match.route
        let facts = [PlannerRoutesText.kind(route, finder: finder), graded ? route.hardest.map { PlannerRoutesText.grade($0, mtb: route.kind == .mtb) } : nil,
                     "\(OBCFormat.distance(meters: match.distanceM)) away"].compactMap { $0 }.joined(separator: " · ")
        return HStack(spacing: 12) {
            PlannerRouteNumber(number: number, rank: route.rank)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    PlannerRouteRefBadge(route: route)
                    Text(route.title).font(.system(.body, weight: .semibold)).foregroundStyle(OBCTheme.ink).lineLimit(2)
                }
                Text(facts).font(.subheadline.monospacedDigit()).foregroundStyle(OBCTheme.secondary)
            }
            Spacer(minLength: 8)
            VStack(alignment: .trailing, spacing: 2) {
                Text(OBCFormat.distance(meters: route.length_m)).font(.subheadline.weight(.semibold).monospacedDigit())
                Text.obcClimb(meters: route.ascent_m).font(.caption.monospacedDigit()).foregroundStyle(OBCTheme.secondary)
            }
        }
        .frame(minHeight: 56).contentShape(Rectangle())
    }
}

/// One route: profile, figures, grade mix, where it starts, and "Plan this route" at the foot.
struct PlannerRouteDetail: View {
    let finder: PlannerRouteFinder
    let detail: PlannerRouteFinder.Detail
    /// The start and finish of the plan, by the nearest places of the loaded map.
    let ends: (start: String?, finish: String?)
    @Binding var fraction: Double?
    let onSelect: (CatalogRecord) -> Void

    var body: some View {
        let route = detail.route
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    PlannerRouteRefBadge(route: route)
                    Text(route.title).font(.system(.title3, weight: .semibold))
                }
                Text([route.loop ? "Signed loop" : PlannerRoutesText.kind(route, finder: finder), PlannerRoutesText.network(route),
                      detail.distanceM.map { "\(OBCFormat.distance(meters: $0)) from \(finder.start?.name ?? "the start")" }]
                    .compactMap { $0 }.joined(separator: " · "))
                    .font(.subheadline.monospacedDigit()).foregroundStyle(OBCTheme.secondary)
            }
            if let profile = finder.preview?.profile {
                PlannerPreviewProfile(profile: profile, height: 64, visibleRange: nil, selectedFraction: $fraction)
            }
            OBCStatStrip(figures(route))
            if let line = endsLine(route) {
                Label(line, systemImage: route.loop ? "arrow.triangle.2.circlepath" : "flag").font(.subheadline)
            }
            if let grades = route.grades_m, route.kind == .mtb || route.kind == .hiking || route.kind == .foot {
                gradeMix(grades, kind: route.kind)
            }
            if let family = detail.family, let stages = detail.stages { stageList(family, stages: stages, current: route) }
            if let description = route.description { Text(description).font(.subheadline) }
            if route.operator != nil || PlannerRoutesText.website(route.website) != nil {
                OBCGroupedSection {
                    if let name = route.operator {
                        OBCListRow(icon: "person.2", label: name, showsDivider: PlannerRoutesText.website(route.website) != nil)
                    }
                    if let url = PlannerRoutesText.website(route.website) {
                        Link(destination: url) {
                            OBCListRow(icon: "safari", label: url.host()?.replacingOccurrences(of: "www.", with: "") ?? url.absoluteString,
                                       showsChevron: true, showsDivider: false)
                        }.buttonStyle(.plain)
                    }
                }
            }
        }
    }

    private func figures(_ route: CatalogRecord) -> [OBCStat] {
        // The routed plan gives every figure once it is ready, so the time never belongs to other figures.
        let path = finder.preview?.path
        let heights = path?.points.compactMap(\.elevationMeters) ?? []
        let descent = path == nil ? route.descent_m : zip(heights, heights.dropFirst()).reduce(0) { $0 + max(0, $1.0 - $1.1) }
        var stats = [OBCStat(value: OBCFormat.distance(meters: path?.distance ?? route.length_m), key: "Distance"),
                     OBCStat(value: "\(Int((path?.ascent ?? route.ascent_m).rounded())) m", key: "Ascent"),
                     OBCStat(value: "\(Int(descent.rounded())) m", key: "Descent")]
        if let path { stats.append(OBCStat(value: OBCFormat.estimatedClock(path.seconds), key: "Time")) }
        return stats
    }

    private func endsLine(_ route: CatalogRecord) -> String? {
        guard let start = ends.start else { return nil }
        if route.loop { return "Starts and ends at \(start)" }
        return ends.finish.map { "\(start) → \($0)" }
    }

    private func gradeMix(_ grades: [Double], kind: CatalogRecord.Kind) -> some View {
        let parts = grades.enumerated().filter { $0.element > 0 }
        return VStack(alignment: .leading, spacing: 6) {
            GeometryReader { geometry in
                HStack(spacing: 1) {
                    ForEach(parts, id: \.offset) { part in
                        OBCTheme.gradeBands[min(part.offset, 4)]
                            .frame(width: max(2, geometry.size.width * part.element / max(1, grades.reduce(0, +))))
                    }
                }
            }.frame(height: 8).clipShape(Capsule())
            HStack(spacing: 12) {
                ForEach(parts, id: \.offset) { part in
                    Text("\(PlannerRoutesText.grade(part.offset, mtb: kind == .mtb)) \(OBCFormat.distance(meters: part.element))")
                }
            }.font(.caption.monospacedDigit()).foregroundStyle(OBCTheme.secondary)
        }
        .accessibilityElement(children: .combine)
    }

    private func stageList(_ family: CatalogRecord, stages: [CatalogRecord], current: CatalogRecord) -> some View {
        OBCGroupedSection("\(family.title) stages") {
            ForEach(Array(stages.enumerated()), id: \.element.id) { index, stage in
                OBCListRow(label: "\(index + 1). \(stage.title)\(stage.id == current.id ? " · this stage" : "")",
                           value: OBCFormat.distance(meters: stage.length_m), showsChevron: stage.id != current.id,
                           showsDivider: index < stages.count - 1 || current.stage != nil) {
                    if stage.id != current.id { onSelect(stage) }
                }
            }
            if current.stage != nil {
                OBCListRow(label: "Whole \(family.title) · \(stages.count) stages", value: OBCFormat.distance(meters: family.length_m),
                           showsChevron: true, showsDivider: false) { onSelect(family) }
            }
        }
    }

}

/// "Plan this route" at the foot of the detail sheet, or why the route cannot be planned.
struct PlannerRoutePlanFoot: View {
    let finder: PlannerRouteFinder
    let onPlan: (CatalogRecord, RoutePlan) -> Void

    var body: some View {
        switch finder.plan {
        case .ready(let plan):
            Button("Plan this route") { if let route = finder.detail?.route { onPlan(route, plan) } }
                .buttonStyle(.obcPrimary).accessibilityIdentifier("planner.routes.plan")
        case .loading:
            Button("Loading the stages…") {}.buttonStyle(.obcPrimary).disabled(true)
        case .tooLong: note("This route is too long for one plan. Choose a stage to plan it.")
        case .failed: note("The stages of this route could not load. Choose a stage to plan it.")
        case .invalid: note("This route cannot be planned.")
        }
    }

    private func note(_ text: String) -> some View {
        Text(text).font(.subheadline).foregroundStyle(OBCTheme.secondary).frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// The filter form, full screen from the summary line. The filters apply with the amber action.
struct PlannerRouteFiltersPage: View {
    let finder: PlannerRouteFinder
    let activity: RouteActivity
    let onClose: () -> Void
    @State private var draft: PlannerRouteFilters
    @State private var count: Int?

    init(finder: PlannerRouteFinder, activity: RouteActivity, onClose: @escaping () -> Void) {
        self.finder = finder; self.activity = activity; self.onClose = onClose
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
                    if PlannerRoutesText.graded(activity) {
                        section("Hardest part") {
                            PlannerGradeRange(range: $draft.hardest, mtb: activity == .mtb)
                            let reading = PlannerRoutesText.reading(draft.hardest, mtb: activity == .mtb)
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
                count = try await finder.count(draft, activity: activity)
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

/// A range over four grades: T1–T4, or S0–S3 for mountain bike. A tap or a drag moves the nearer end to the grade under the finger.
struct PlannerGradeRange: View {
    @Binding var range: ClosedRange<Int>
    let mtb: Bool
    /// Which end the current touch moves, fixed when it starts.
    @State private var movesLower: Bool?

    var body: some View {
        GeometryReader { geometry in
            let step = geometry.size.width / 4
            HStack(spacing: 0) {
                ForEach(0..<4, id: \.self) { grade in
                    let on = range.contains(grade)
                    Text(PlannerRoutesText.grade(grade, mtb: mtb)).font(.subheadline.weight(.semibold).monospacedDigit())
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
        .accessibilityValue(PlannerRoutesText.reading(range, mtb: mtb).0)
        .accessibilityAdjustableAction { direction in
            let upper = min(3, max(range.lowerBound, range.upperBound + (direction == .increment ? 1 : -1)))
            range = range.lowerBound...upper
        }
    }
}
#endif
