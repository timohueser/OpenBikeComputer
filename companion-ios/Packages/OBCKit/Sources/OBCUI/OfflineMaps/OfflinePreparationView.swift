import SwiftUI

struct OfflinePreparationView: View {
    @Bindable var model: OfflineMapsModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Checking map size").font(.headline)
                Spacer()
                if let started = model.preparationStarted {
                    TimelineView(.periodic(from: started, by: 1)) { context in
                        let elapsed = max(0, Int(context.date.timeIntervalSince(started)))
                        Text(String(format: "%d:%02d", elapsed / 60, elapsed % 60))
                            .font(.footnote.monospacedDigit()).foregroundStyle(OBCTheme.secondary)
                    }
                }
            }
            ProgressView(value: model.fraction).tint(OBCTheme.secondary)
                .accessibilityLabel("Map size check")
            Text(model.status).font(.subheadline).accessibilityIdentifier("offline.preparationStatus")
            Text("You can review the coverage and size before downloading.")
                .font(.footnote).foregroundStyle(OBCTheme.secondary)
            Button(model.isStopping ? "Stopping…" : "Cancel") { model.stop() }
                .buttonStyle(.obcGhost).disabled(model.isStopping)
        }
    }
}
