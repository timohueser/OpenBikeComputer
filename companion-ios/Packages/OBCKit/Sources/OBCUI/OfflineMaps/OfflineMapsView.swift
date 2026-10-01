#if os(iOS)
import OBCPlanner
import SwiftUI

public struct OfflineMapsView: View {
    @Bindable private var model: OfflineMapsModel
    @State private var selecting: Bool
    @State private var mobilePrompt = false
    @State private var deleting: OfflineMap?
    private let initialBounds: [Double]?

    public init(model: OfflineMapsModel, initialBounds: [Double]? = nil, selecting: Bool = false) {
        self.model = model; self.initialBounds = initialBounds
        _selecting = State(initialValue: selecting)
    }

    public var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                Text("Your offline maps").font(.title2.bold())
                Text("Plan routes, find places and view maps without a connection.")
                    .foregroundStyle(OBCTheme.secondary)
                if model.isBusy || model.quote != nil { transfer }
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
            OfflineAreaView(model: model, initialBounds: initialBounds) { selecting = false }
        }
        .confirmationDialog("Use mobile data?", isPresented: $mobilePrompt, titleVisibility: .visible) {
            Button("Wait for Wi-Fi") { model.download(allowMobileData: false) }
            Button("Use mobile data") { model.download(allowMobileData: true) }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This download uses \(OfflineMapsModel.bytes(model.quote?.transferBytes ?? 0)) of your mobile plan.")
        }
        .confirmationDialog("Delete \(deleting?.name ?? "map")?", isPresented: Binding(
            get: { deleting != nil }, set: { if !$0 { deleting = nil } }), titleVisibility: .visible) {
            Button("Delete map", role: .destructive) {
                if let map = deleting { Task { await model.remove(map) } }
                deleting = nil
            }
        } message: { Text("You will need a connection to plan in this area until you download it again.") }
    }

    @ViewBuilder private var transfer: some View {
        if let quote = model.quote {
            VStack(alignment: .leading, spacing: 14) {
                Text(quote.map.name).font(.headline)
                if model.isDownloading {
                    ProgressView(value: model.fraction)
                    Text(model.status).font(.subheadline).accessibilityIdentifier("offline.progress")
                    Text("\(OfflineMapsModel.bytes(model.transferred)) of \(OfflineMapsModel.bytes(model.transferTotal))")
                        .font(.footnote.monospacedDigit()).foregroundStyle(OBCTheme.secondary)
                    Text("Keep OBC open while downloading. You can pause and resume later.")
                        .font(.footnote).foregroundStyle(OBCTheme.secondary)
                    Button("Pause download") { model.stop() }.buttonStyle(.obcGhost)
                } else {
                    OfflineAreaMap(focus: quote.map.bounds, coverage: nil, selecting: false, onBounds: { _ in })
                        .frame(height: 160).clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
                        .allowsHitTesting(false).accessibilityLabel("Selected offline map coverage")
                    OBCGroupedSection {
                        sizeRow("Download", quote.transferBytes)
                        sizeRow("On this iPhone", quote.map.installedBytes)
                        sizeRow("Free space needed", quote.requiredBytes)
                    }
                    Text("Free space includes temporary files. Maps are ready after all files are checked.")
                        .font(.footnote).foregroundStyle(OBCTheme.secondary)
                    if let available = model.availableBytes, available < quote.requiredBytes {
                        Text("Not enough space. Choose a smaller area or delete a downloaded map.")
                            .foregroundStyle(OBCTheme.danger)
                    } else {
                        Button(model.status == "Download paused" ? "Resume download" : "Download map") {
                            if model.needsMobileConsent { mobilePrompt = true }
                            else { model.download(allowMobileData: false) }
                        }.buttonStyle(.obcPrimary).disabled(model.isBusy || model.availableBytes == nil)
                            .accessibilityIdentifier("offline.download")
                        Text("Wi-Fi only unless you allow mobile data for this download.")
                            .font(.footnote).foregroundStyle(OBCTheme.secondary)
                    }
                    Button("Cancel download") { Task { await model.discard() } }.buttonStyle(.obcGhost)
                }
            }
        } else if model.isBusy {
            OfflinePreparationView(model: model)
        }
    }

    private func sizeRow(_ title: String, _ bytes: Int64) -> some View {
        HStack { Text(title); Spacer(); Text(OfflineMapsModel.bytes(bytes)).monospacedDigit() }.padding(16)
    }
}
#endif
