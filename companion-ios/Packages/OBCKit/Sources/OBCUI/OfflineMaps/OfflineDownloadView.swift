#if os(iOS)
import OBCPlanner
import SwiftUI

struct OfflineDownloadView: View {
    @Bindable var model: OfflineMapsModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if model.quote != nil || model.isBusy {
                    OfflineDownloadContent(model: model, onCancel: { dismiss() })
                } else if model.status == "Ready offline" {
                    ContentUnavailableView("Map ready offline", systemImage: "checkmark.circle",
                        description: Text("Plan routes, find places and view maps without a connection."))
                }
                if let error = model.error {
                    Text(error).foregroundStyle(OBCTheme.danger).accessibilityIdentifier("offline.error")
                }
            }.padding(20)
        }
        .background(OBCTheme.page).foregroundStyle(OBCTheme.ink).tint(OBCTheme.tint)
        .navigationTitle(model.isDownloading ? "Downloading map" : "Offline download")
        .navigationBarTitleDisplayMode(.inline)
        .task { await model.refresh() }
    }
}

struct OfflineDownloadContent: View {
    @Bindable var model: OfflineMapsModel
    var onCancel: () -> Void = {}
    @State private var mobilePrompt = false

    @ViewBuilder var body: some View {
        if let quote = model.quote {
            VStack(alignment: .leading, spacing: 14) {
                if model.hasStartedDownload {
                    Text(quote.map.name).font(.headline)
                } else {
                    HStack {
                        Text("Name").foregroundStyle(OBCTheme.secondary)
                        TextField("Map name", text: Binding(get: { model.quote?.map.name ?? "" }, set: { model.rename($0) }))
                            .multilineTextAlignment(.trailing).accessibilityLabel("Map name")
                            .accessibilityIdentifier("offline.name")
                    }.padding(12).background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
                }
                if model.isDownloading {
                    ProgressView(value: model.fraction)
                    Text(model.status).font(.subheadline).accessibilityIdentifier("offline.progress")
                    Text("\(OfflineMapsModel.bytes(model.transferred)) of \(OfflineMapsModel.bytes(model.transferTotal))")
                        .font(.footnote.monospacedDigit()).foregroundStyle(OBCTheme.secondary)
                    Text("Keep OBC open while downloading. You can pause and resume later.")
                        .font(.footnote).foregroundStyle(OBCTheme.secondary)
                    Button("Pause download") { model.stop() }.buttonStyle(.obcGhost)
                } else {
                    OfflineAreaMap(selection: .constant(OfflineAreaSelection(quote.map.bounds)), editable: false)
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
                        Button(model.hasStartedDownload ? "Resume download" : "Download map") {
                            if model.needsMobileConsent { mobilePrompt = true }
                            else { model.download(allowMobileData: false) }
                        }.buttonStyle(.obcPrimary).disabled(model.isBusy || model.availableBytes == nil
                                || quote.map.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                            .accessibilityIdentifier("offline.download")
                        Text("Wi-Fi only unless you allow mobile data for this download.")
                            .font(.footnote).foregroundStyle(OBCTheme.secondary)
                    }
                    Button("Cancel download") { Task { await model.discard(); if model.quote == nil { onCancel() } } }.buttonStyle(.obcGhost)
                }
            }
            .confirmationDialog("Use mobile data?", isPresented: $mobilePrompt, titleVisibility: .visible) {
                Button("Wait for Wi-Fi") { model.download(allowMobileData: false) }
                Button("Use mobile data") { model.download(allowMobileData: true) }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("This download uses \(OfflineMapsModel.bytes(model.quote?.transferBytes ?? 0)) of your mobile plan.")
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
