import SwiftUI

/// The developer sheet behind the gear: what is on the phone, what the host is doing, and the two
/// actions that are not a touch pad.
struct DevSheet: View {
    @Bindable var controller: HostController
    @Environment(\.dismiss) private var dismiss
    @State private var inbox: [URL] = []
    @State private var rides: [URL] = []

    var body: some View {
        NavigationStack {
            List {
                Section("Panel") {
                    Picker("Pixel scale", selection: $controller.pixelScale) {
                        ForEach(1...4, id: \.self) { scale in Text("\(scale)×").tag(scale) }
                    }
                    .pickerStyle(.segmented)
                    LabeledContent("Screen", value: controller.screen.isEmpty ? "—" : controller.screen)
                }

                #if DEBUG
                    PretendLocationSection(controller: controller)
                #endif

                Section("Documents") {
                    if inbox.isEmpty {
                        Text("Nothing dropped yet").foregroundStyle(.secondary)
                    }
                    ForEach(inbox, id: \.self, content: row)
                }

                Section("Rides") {
                    if rides.isEmpty {
                        Text("No exported rides").foregroundStyle(.secondary)
                    }
                    ForEach(rides, id: \.self) { ride in
                        ShareLink(item: ride) {
                            Label(ride.lastPathComponent, systemImage: "square.and.arrow.up")
                        }
                    }
                }

                Section("Card") {
                    LabeledContent("Last error", value: controller.hostError.isEmpty ? "—" : controller.hostError)
                    Button("Reset card", role: .destructive) {
                        controller.resetCard()
                        dismiss()
                    }
                    // The detached copy is writing this card.
                    .disabled(controller.isImporting)
                }
            }
            .navigationTitle("Developer")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
        .onAppear(perform: reload)
    }

    private func row(_ file: URL) -> some View {
        HStack {
            Text(file.lastPathComponent).lineLimit(1).truncationMode(.middle)
            Spacer()
            if file.pathExtension.caseInsensitiveCompare("obcm") == .orderedSame {
                Button("Use as map") {
                    controller.useAsMap(file)
                    dismiss()
                }
                .buttonStyle(.borderless)
            } else {
                Button("Import route") { controller.importRoute(file) }
                    .buttonStyle(.borderless)
            }
        }
        .swipeActions {
            Button("Delete", role: .destructive) {
                controller.delete(file)
                reload()
            }
            // The detached copy may be reading this file.
            .disabled(controller.isImporting)
        }
    }

    private func reload() {
        inbox = controller.inbox()
        rides = controller.exportedRides()
    }
}
