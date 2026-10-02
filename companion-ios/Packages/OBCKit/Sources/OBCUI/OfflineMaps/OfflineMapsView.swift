#if os(iOS)
import OBCPlanner
import SwiftUI

public struct OfflineMapsView: View {
    @Bindable private var model: OfflineMapsModel
    @State private var selecting = false
    @State private var deleting: OfflineMap?

    public init(model: OfflineMapsModel) { self.model = model }

    public var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                Text("Your offline maps").font(.title2.bold())
                Text("Plan routes, find places and view maps without a connection.")
                    .foregroundStyle(OBCTheme.secondary)
                if model.isBusy || model.quote != nil { OfflineDownloadContent(model: model) }
                if let error = model.error {
                    Text(error).foregroundStyle(OBCTheme.danger).accessibilityIdentifier("offline.error")
                }
                if model.maps.isEmpty && !model.isBusy && model.quote == nil {
                    ContentUnavailableView("No downloaded maps", systemImage: "map",
                        description: Text("Choose an area before you set off."))
                } else if !model.maps.isEmpty {
                    OBCGroupedSection {
                        ForEach(model.maps) { map in
                            HStack(spacing: 12) {
                                VStack(alignment: .leading, spacing: 4) {
                                    Text(map.name).font(.headline)
                                    Text("Ready offline · \(OfflineMapsModel.bytes(map.installedBytes))")
                                        .font(.subheadline).foregroundStyle(OBCTheme.secondary)
                                }
                                Spacer()
                                Button("Delete map", systemImage: "trash") { deleting = map }
                                    .labelStyle(.iconOnly).frame(width: 44, height: 44).disabled(model.isBusy)
                            }.padding(16)
                        }
                    }
                }
                if !model.isBusy && model.quote == nil {
                    Button("Download a map", systemImage: "arrow.down.to.line") {
                        model.clearSelection(); selecting = true
                    }.buttonStyle(.obcPrimary).accessibilityIdentifier("offline.add")
                }
                if let bytes = model.availableBytes {
                    Text("\(OfflineMapsModel.bytes(bytes)) available on this iPhone")
                        .font(.footnote).foregroundStyle(OBCTheme.secondary)
                }
            }.padding(20)
        }
        .background(OBCTheme.page).foregroundStyle(OBCTheme.ink).tint(OBCTheme.tint)
        .navigationTitle("Offline maps").navigationBarTitleDisplayMode(.inline)
        .task { await model.refresh() }
        .navigationDestination(isPresented: $selecting) {
            OfflineAreaView(model: model)
        }
        .confirmationDialog("Delete \(deleting?.name ?? "map")?", isPresented: Binding(
            get: { deleting != nil }, set: { if !$0 { deleting = nil } }), titleVisibility: .visible) {
            Button("Delete map", role: .destructive) {
                if let map = deleting { Task { await model.remove(map) } }
                deleting = nil
            }
        } message: { Text("You will need a connection to plan in this area until you download it again.") }
    }

}
#endif
