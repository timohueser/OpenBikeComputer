#if DEBUG && os(iOS)
import SwiftUI

private extension PlannerPreviewQueryField {
    var ink: Color {
        switch self {
        case .what, .name: Color(day: 0x3B654C, tent: 0xAED0B5)
        case .area: Color(day: 0x365F84, tent: 0xA9CCE9)
        case .radius: OBCTheme.secondary
        }
    }
    var symbol: String {
        switch self { case .what, .name: "magnifyingglass"; case .area: "mappin"; case .radius: "slider.horizontal.3" }
    }
}

struct PlannerPreviewQuerySummary: View {
    let request: PlannerPreviewPlaceQuery
    var edited = false
    let onEdit: (PlannerPreviewQueryField) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(edited ? "Edited request" : "Understood request")
                .font(.caption).foregroundStyle(OBCTheme.secondary)
            PlannerPreviewQueryRows {
                ForEach(request.fields) { field in
                    Button { onEdit(field) } label: {
                        HStack(spacing: 6) {
                            Image(systemName: field.symbol)
                            Text(request.label(for: field)).multilineTextAlignment(.leading)
                            Image(systemName: "chevron.down").imageScale(.small)
                        }
                        .font(.subheadline)
                        .foregroundStyle(field.ink)
                        .padding(.horizontal, 12).padding(.vertical, 7)
                        .background(field.ink.opacity(0.11), in: Capsule())
                        .frame(minHeight: 44)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("\(field.title): \(request.label(for: field))")
                    .accessibilityHint("Edit this part of your search")
                    .accessibilityIdentifier("planner.query.\(field.rawValue)")
                }
            }
        }
    }
}

struct PlannerPreviewQueryEditor: View {
    let field: PlannerPreviewQueryField
    let hasRoute: Bool
    let routeLengthMeters: Double
    let onApply: (PlannerPreviewPlaceQuery) -> Void
    let onCancel: () -> Void
    @State private var draft: PlannerPreviewPlaceQuery

    init(field: PlannerPreviewQueryField, request: PlannerPreviewPlaceQuery, hasRoute: Bool,
         routeLengthMeters: Double, onApply: @escaping (PlannerPreviewPlaceQuery) -> Void, onCancel: @escaping () -> Void) {
        self.field = field; self.hasRoute = hasRoute; self.routeLengthMeters = routeLengthMeters
        self.onApply = onApply; self.onCancel = onCancel
        var value = request
        if field == .radius && value.radiusMeters == nil { value.radiusMeters = 2_000 }
        _draft = State(initialValue: value)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(field.title).font(.headline)
                Spacer()
                Button(action: onCancel) { Image(systemName: "xmark").frame(width: 44, height: 44) }
                    .accessibilityLabel("Close filter editor")
            }
            .foregroundStyle(field.ink)
            .padding(.leading, 14).padding(.trailing, 4)
            .background(field.ink.opacity(0.11))
            VStack(alignment: .leading, spacing: 12) { fields }
                .padding(14)
            Divider().overlay(OBCTheme.hairline)
            HStack {
                if field == .radius {
                    Button("Remove filter") { draft.radiusMeters = nil; onApply(draft) }
                        .foregroundStyle(OBCTheme.secondary)
                } else {
                    Button("Cancel", action: onCancel).foregroundStyle(OBCTheme.secondary)
                }
                Spacer()
                Button("Apply", systemImage: "checkmark") { onApply(draft) }
                    .buttonStyle(.borderedProminent).tint(field.ink)
                    .foregroundStyle(OBCTheme.surface).buttonBorderShape(.capsule)
                    .disabled(field == .what && draft.kinds.isEmpty || field == .name && draft.name.trimmingCharacters(in: .whitespaces).isEmpty)
                    .accessibilityIdentifier("planner.query.apply")
            }
            .font(.subheadline.weight(.semibold)).frame(minHeight: 44).padding(10)
        }
        .background(OBCTheme.surface)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(OBCTheme.hairlineStrong, lineWidth: 1))
    }

    @ViewBuilder private var fields: some View {
        switch field {
        case .what:
            ForEach(PlannerPreviewPlaceQuery.placeTypes, id: \.rawValue) { kind in
                Toggle(isOn: Binding(get: { draft.kinds.contains(kind) }, set: { selected in
                    if selected { draft.kinds.insert(kind) } else { draft.kinds.remove(kind) }
                })) { Label(kind.title, systemImage: kind.symbol) }
                    .tint(field.ink).frame(minHeight: 44)
            }
        case .name:
            TextField("Place name", text: $draft.name).textFieldStyle(.roundedBorder).frame(minHeight: 44)
        case .area:
            ForEach(PlannerPreviewPlaceQuery.Area.allCases.filter { hasRoute || $0 == .view }) { area in
                Button {
                    draft.area = area
                    if area == .view { draft.radiusMeters = nil }
                    if area == .section {
                        draft.fromMeters = min(draft.fromMeters, routeLengthMeters)
                        draft.toMeters = max(draft.fromMeters, min(draft.toMeters ?? 10_000, routeLengthMeters))
                    }
                } label: {
                    HStack {
                        Text(area.title)
                        Spacer()
                        Image(systemName: draft.area == area ? "checkmark.circle.fill" : "circle")
                    }
                    .frame(minHeight: 44).contentShape(Rectangle())
                }
                .buttonStyle(.plain).foregroundStyle(draft.area == area ? field.ink : OBCTheme.ink)
            }
            if draft.area == .section {
                Stepper(value: $draft.fromMeters, in: 0...max(0, (draft.toMeters ?? routeLengthMeters) - 1_000), step: 1_000) {
                    Text("From \(draft.fromMeters / 1_000, specifier: "%.0f") km")
                }
                Stepper(value: Binding(get: { draft.toMeters ?? routeLengthMeters }, set: { draft.toMeters = $0 }),
                        in: min(draft.fromMeters + 1_000, routeLengthMeters)...routeLengthMeters, step: 1_000) {
                    Text("To \((draft.toMeters ?? routeLengthMeters) / 1_000, specifier: "%.0f") km")
                }
            }
        case .radius:
            Text("Maximum distance from the route")
                .font(.subheadline).foregroundStyle(OBCTheme.secondary)
            ForEach([100.0, 500, 1_000, 2_000, 5_000], id: \.self) { meters in
                Button { draft.radiusMeters = meters } label: {
                    HStack {
                        Text(meters < 1_000 ? "\(Int(meters)) m" : "\(Int(meters / 1_000)) km")
                        Spacer()
                        Image(systemName: draft.radiusMeters == meters ? "checkmark.circle.fill" : "circle")
                    }
                    .frame(minHeight: 44).contentShape(Rectangle())
                }
                .buttonStyle(.plain).foregroundStyle(draft.radiusMeters == meters ? field.ink : OBCTheme.ink)
            }
        }
    }
}

private struct PlannerPreviewQueryRows: Layout {
    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        arrangement(subviews, width: proposal.width ?? 350).size
    }
    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let rows = arrangement(subviews, width: bounds.width)
        for (view, frame) in zip(subviews, rows.frames) {
            view.place(at: CGPoint(x: bounds.minX + frame.minX, y: bounds.minY + frame.minY),
                       proposal: ProposedViewSize(frame.size))
        }
    }
    private func arrangement(_ subviews: Subviews, width: CGFloat) -> (frames: [CGRect], size: CGSize) {
        var x: CGFloat = 0, y: CGFloat = 0, rowHeight: CGFloat = 0
        var frames: [CGRect] = []
        for view in subviews {
            let size = view.sizeThatFits(ProposedViewSize(width: width, height: nil))
            if x > 0 && x + size.width > width { x = 0; y += rowHeight + 4; rowHeight = 0 }
            frames.append(CGRect(origin: CGPoint(x: x, y: y), size: size))
            x += size.width + 6; rowHeight = max(rowHeight, size.height)
        }
        return (frames, CGSize(width: width, height: y + rowHeight))
    }
}
#endif
