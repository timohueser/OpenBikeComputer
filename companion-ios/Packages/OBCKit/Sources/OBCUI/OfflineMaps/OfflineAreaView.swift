#if os(iOS)
import OBCPlanner
import SwiftUI

struct OfflineAreaView: View {
    @Bindable var model: OfflineMapsModel
    private let onClose: (() -> Void)?
    @Environment(\.obcPlannerSource) private var source
    @State private var selection: OfflineAreaSelection?
    @State private var fitRevision = 0
    @State private var reviewing = false
    @State private var preparing = false
    @State private var query = ""
    @State private var places: [PlannerPlace] = []
    @State private var searching = false
    @State private var name = "Map area"

    init(model: OfflineMapsModel, initialBounds: [Double]? = nil, onClose: (() -> Void)? = nil) {
        self.model = model
        self.onClose = onClose
        _selection = State(initialValue: initialBounds.flatMap(OfflineAreaSelection.init))
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Image(systemName: "magnifyingglass")
                TextField("Find an area", text: $query).autocorrectionDisabled()
                    .accessibilityIdentifier("offline.search")
                if searching { ProgressView() }
            }.padding(12).background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
                .padding(.horizontal, 16).padding(.bottom, 10)
            OfflineAreaMap(
                selection: $selection, coverage: model.coverage, fitRevision: fitRevision
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .allowsHitTesting(!preparing)
            .overlay(alignment: .topTrailing) {
                Button { fitRevision += 1 } label: {
                    Label("Show selected area", systemImage: "arrow.up.left.and.arrow.down.right")
                        .labelStyle(.iconOnly).frame(width: 44, height: 44)
                }
                .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
                .padding(12).disabled(selection == nil)
                .accessibilityIdentifier("offline.fit")
            }
            .overlay(alignment: .top) {
                if !query.isEmpty && !places.isEmpty {
                    ScrollView {
                        VStack(spacing: 0) {
                            ForEach(places) { place in
                                Button {
                                    name = place.name + " area"
                                    selection = OfflineAreaSelection([
                                        max(-180, place.lon - 0.15), max(-85, place.lat - 0.1),
                                        min(180, place.lon + 0.15), min(85, place.lat + 0.1)])
                                    fitRevision += 1
                                    query = ""
                                    places = []
                                } label: {
                                    HStack {
                                        Text(place.name)
                                        Spacer()
                                        Text(place.city).foregroundStyle(OBCTheme.secondary)
                                    }
                                    .frame(minHeight: 44).padding(.horizontal, 16)
                                }
                                Divider()
                            }
                        }
                    }.frame(maxHeight: 220).background(OBCTheme.surface)
                }
            }
            VStack(alignment: .leading, spacing: 12) {
                if preparing {
                    OfflinePreparationView(model: model)
                } else {
                    Label("Drag a corner to resize", systemImage: "arrow.up.left.and.arrow.down.right")
                        .font(.subheadline)
                    Text("Pan and zoom freely. The shaded area shows the full download coverage.")
                        .font(.footnote).foregroundStyle(OBCTheme.secondary)
                    if let selection, let coverage = model.coverage, !coverage.contains(selection.bounds) {
                        Text("Keep the box inside available map coverage.").font(.footnote).foregroundStyle(OBCTheme.danger)
                    }
                    if let error = model.error {
                        Text(error).font(.footnote).foregroundStyle(OBCTheme.danger)
                        if model.coverage == nil {
                            Button("Try again") { Task { await loadCoverage() } }.buttonStyle(.obcGhost)
                        }
                    }
                    if model.isBusy || model.quote != nil {
                        Button("Show current download") { reviewing = true }
                            .buttonStyle(.obcPrimary)
                    } else {
                        Button("Review download") {
                            guard let selection else { return }
                            preparing = true
                            model.prepare(bounds: selection.bounds, name: name.trimmingCharacters(in: .whitespacesAndNewlines))
                        }.buttonStyle(.obcPrimary)
                            .disabled(selection.map { model.coverage?.contains($0.bounds) != true } ?? true)
                            .accessibilityIdentifier("offline.prepare")
                    }
                }
            }.padding(16).background(OBCTheme.page)

        }
        .background(OBCTheme.page).foregroundStyle(OBCTheme.ink).tint(OBCTheme.tint)
        .navigationTitle("Select area").navigationBarTitleDisplayMode(.inline)
        .toolbar { closeToolbar }
        .task {
            await model.refresh()
            await loadCoverage()
        }
        .navigationDestination(isPresented: $reviewing) {
            OfflineDownloadView(model: model)
                .toolbar { closeToolbar }
        }
        .onChange(of: model.isBusy) { _, busy in
            guard preparing, !busy else { return }
            preparing = false
            if model.quote != nil { reviewing = true }
        }
        .onChange(of: reviewing) { _, shown in
            if !shown {
                if let quote = model.quote { name = quote.map.name }
                model.clearSelection()
            }
        }
        .onDisappear {
            if preparing { model.stop() }
        }
        .task(id: query) {
            places = []
            guard query.count > 1 else { return }
            searching = true
            defer { searching = false }
            do {
                try await Task.sleep(for: .milliseconds(350))
                let release = try await source.release()
                let results = try await source.search(PlannerSearchQuery(text: query, view: selection?.bounds), release: release)
                try Task.checkCancellation()
                places = Array(results.prefix(5))
            } catch is CancellationError {} catch { model.error = error.localizedDescription }
        }
    }

    private func loadCoverage() async {
        await model.loadCoverage()
        if selection == nil, let bounds = model.coverage?.bounds { selection = OfflineAreaSelection(bounds) }
    }

    @ToolbarContentBuilder private var closeToolbar: some ToolbarContent {
        if let onClose {
            ToolbarItem(placement: .topBarTrailing) {
                Button("Done") {
                    model.clearSelection()
                    onClose()
                }.accessibilityIdentifier("offline.done")
            }
        }
    }
}

#endif
