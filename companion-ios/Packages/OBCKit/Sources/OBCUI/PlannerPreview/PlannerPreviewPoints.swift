#if os(iOS)
import SwiftUI

struct PlannerPreviewPoints: View {
    let model: PlannerPreviewModel
    let onEdit: (PlannerPreviewPoint) -> Void
    let onAdd: () -> Void
    let onReverse: () -> Void
    let onExample: () -> Void
    let onNew: () -> Void

    var body: some View {
        List {
            Section {
                ForEach(Array(model.points.enumerated()), id: \.element.id) { index, point in
                    Button { onEdit(point) } label: {
                        row(point, role: role(at: index), symbol: index == 0 ? "play.fill" : index == model.points.count - 1 ? "flag.checkered" : point.kind.symbol)
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("planner.point.\(point.id)")
                }
                .onMove { model.movePoint(fromOffsets: $0, toOffset: $1) }
            }
            .listRowBackground(OBCTheme.surface)

            if !model.markers.isEmpty {
                Section("Map markers") {
                    ForEach(model.markers) { point in
                        Button { onEdit(point) } label: { row(point, role: "Marker", symbol: "mappin") }
                            .buttonStyle(.plain)
                    }
                }.listRowBackground(OBCTheme.surface)
            }
            Section {
                Button(action: onAdd) { Label("Add a point", systemImage: "plus") }.frame(minHeight: 44)
                if model.hasRoute {
                    Button("Reverse route", systemImage: "arrow.up.arrow.down", action: onReverse).frame(minHeight: 44)
                }
                Menu("Route") {
                    Button("Load example route", action: onExample)
                    Button("Start a new route", action: onNew)
                }.frame(minHeight: 44)
            } footer: {
                Text("Routes need an internet connection and points inside the available map region.")
            }.listRowBackground(OBCTheme.surface)
        }
        .environment(\.editMode, .constant(.active))
        .listStyle(.insetGrouped)
        .contentMargins(.top, 8, for: .scrollContent)
        .scrollContentBackground(.hidden)
        .tint(OBCTheme.tint)
    }

    private func role(at index: Int) -> String {
        if index == 0 { return "Start" }
        if index == model.points.count - 1 { return "Finish" }
        let point = model.points[index]
        return point.id == model.overnightPointID ? "End of day 1" : point.kind.title
    }

    private func row(_ point: PlannerPreviewPoint, role: String, symbol: String) -> some View {
        HStack(spacing: 12) {
            Image(systemName: symbol).foregroundStyle(OBCTheme.secondary).frame(width: 24)
            VStack(alignment: .leading, spacing: 3) {
                Text(role).font(.caption).foregroundStyle(OBCTheme.secondary)
                Text(point.place.name).font(.body).foregroundStyle(OBCTheme.ink)
            }
            Spacer(minLength: 4)
        }.frame(minHeight: 52).contentShape(Rectangle())
    }
}
#endif
