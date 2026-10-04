#if os(iOS)
import OBCDomain
import OBCPlanner
import SwiftUI

/// The words of the Routes view. Only mountain bike has a difficulty filter on the phone.
@MainActor enum PlannerRoutesText {
    static let levels = ["", "Local", "Regional", "National", "International"]
    static func grade(_ index: Int, kind: CatalogRecord.Kind) -> String { kind == .mtb ? "S\(index)" : "T\(index + 1)" }
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
    static func hardest(_ range: ClosedRange<Int>) -> String {
        range.lowerBound == 0 ? "up to S\(range.upperBound)" : range.lowerBound == range.upperBound ? "S\(range.lowerBound)"
            : "S\(range.lowerBound) or harder"
    }
    /// The reading under the hardest-part control, and its detail.
    static func reading(_ range: ClosedRange<Int>) -> (String, String) {
        let (low, high) = (range.lowerBound, range.upperBound)
        let names = (low...high).map { "S\($0)" }
        let list = names.count > 1 ? names.dropLast().joined(separator: ", ") + " or " + names.last! : names[0]
        let reading = low == 0 ? "Up to S\(high)." : high == 3 ? "Must include S\(low) or harder."
            : low == high ? "Hardest part S\(low)." : "Must include S\(low) or harder, up to S\(high)."
        return (reading, low == 0 ? "Routes whose hardest part is \(list). A trail without a grade counts as S0."
                : "Only routes with a part graded \(list).")
    }
    static func summary(_ filters: PlannerRouteFilters, graded: Bool) -> String {
        ["\(Int(filters.radiusKm)) km", shape(filters.shape), range(filters.distanceKm, unit: "km"), range(filters.climbM, unit: "m"),
         graded ? hardest(filters.hardest) : nil].compactMap { $0 }.joined(separator: " · ")
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
    let bike: BikeType
    /// The plan under the view, for the line that leads back to it.
    let plan: (title: String, meters: Double)?
    let onShowPlan: () -> Void
    let onFilters: () -> Void
    let onSelect: (CatalogRecord) -> Void
    let onRetry: () -> Void

    private var graded: Bool { bike == .mtb }
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
                    Text(PlannerRoutesText.summary(finder.filters, graded: graded)).font(.subheadline).foregroundStyle(OBCTheme.ink)
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
            note(finder.offline ? "The routes of this download could not be read." : "The routes could not load. Check your connection and try again.")
            Button("Try again", action: onRetry).frame(minHeight: 44)
        } else if finder.status == .loading && finder.progress.loaded < finder.progress.total {
            note("Loading the routes around \(place) · \(finder.progress.loaded) of \(finder.progress.total) map cells")
        } else if finder.status == .ready && finder.matches.isEmpty {
            empty(filters)
            if finder.offline { note("Only routes inside your download are shown.") }
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
            case .hardest: PlannerRoutesText.hardest(filters.hardest)
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
            note("No signed \(bike == .mtb ? "mountain bike" : "cycling") \(filters.shape == .loop ? "loops" : "routes") within \(Int(filters.radiusKm)) km of \(place). \(count) within \(Int(radius)) km.")
            Button("Search within \(Int(radius)) km") { finder.filters.radiusKm = radius }.buttonStyle(.obcGhost)
        case nil:
            note("No signed \(bike == .mtb ? "mountain bike" : "cycling") \(filters.shape == .loop ? "loops" : "routes") within \(Int(filters.radiusKm)) km of \(place).")
        }
    }

    private func note(_ text: String) -> some View {
        Text(text).font(.subheadline).foregroundStyle(OBCTheme.secondary).fixedSize(horizontal: false, vertical: true)
    }

    private func row(_ match: RouteMatch, number: Int) -> some View {
        let route = match.route
        let facts = [PlannerRoutesText.kind(route, finder: finder), graded ? route.hardest.map { PlannerRoutesText.grade($0, kind: route.kind) } : nil,
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
    let bike: BikeType
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
                    Text("\(PlannerRoutesText.grade(part.offset, kind: kind)) \(OBCFormat.distance(meters: part.element))")
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
#endif
