import SwiftUI
import UIKit

/// The device on the phone: the panel above, the four pads below, and the dev sheet behind the
/// gear. Everything the rider sees inside the panel is the firmware's.
struct DeviceView: View {
    let controller: HostController
    @State private var showsDevSheet = false

    /// The space around the panel and the pads.
    private static let margin: CGFloat = 16

    var body: some View {
        ZStack(alignment: .topTrailing) {
            Color.black.ignoresSafeArea()
            switch controller.state {
            case .needsMap: importPrompt
            case .running: GeometryReader(content: device)
            case .failed(let message): failure(message)
            }
            gear
        }
        .alert(
            "Use as map?", isPresented: binding(for: \.offeredMap), presenting: controller.offeredMap
        ) { map in
            Button("Use as map") { controller.useAsMap(map) }
            Button("Cancel", role: .cancel) {}
        } message: { map in
            Text("\(map.lastPathComponent) replaces the map on the card.")
        }
        .alert("OBC Device", isPresented: binding(for: \.notice), presenting: controller.notice) { _ in
            Button("OK") {}
        } message: { message in
            Text(message)
        }
        .sheet(isPresented: $showsDevSheet) { DevSheet(controller: controller) }
    }

    // MARK: The device

    private func device(_ geometry: GeometryProxy) -> some View {
        let panel = controller.panelSize
        let window = geometry.frame(in: .global).origin
        let left = pixelAligned(window.x + (geometry.size.width - panel.width) / 2) - window.x
        let top = pixelAligned(window.y + Self.margin) - window.y
        return ZStack(alignment: .topLeading) {
            Pads(controller: controller)
                .padding(EdgeInsets(
                    top: top + panel.height + Self.margin, leading: Self.margin,
                    bottom: Self.margin, trailing: Self.margin))
            Panel(controller: controller)
                .frame(width: panel.width, height: panel.height)
                .offset(x: left, y: top)
        }
    }

    /// Put an edge on the physical pixel grid. A panel pixel that starts on a half pixel is
    /// resampled, and the point of an integer scale is that none of them are.
    private func pixelAligned(_ value: CGFloat) -> CGFloat {
        let scale = UIScreen.main.scale
        return (value * scale).rounded() / scale
    }

    // MARK: The other two states

    private var importPrompt: some View {
        VStack(spacing: 14) {
            Image(systemName: "map").font(.largeTitle)
            Text("No map on the card").font(.title2.bold())
            Text("AirDrop an .obcm map to this phone, or open one from Files. Then choose Use as map.")
                .multilineTextAlignment(.center)
                .foregroundStyle(.secondary)
            if controller.isImporting {
                ProgressView("Importing the map…").padding(.top, 8)
            } else {
                Button("Open the file list") { showsDevSheet = true }
                    .buttonStyle(.borderedProminent)
                    .padding(.top, 8)
            }
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func failure(_ message: String) -> some View {
        VStack(spacing: 14) {
            Image(systemName: "exclamationmark.triangle").font(.largeTitle)
            Text("The host did not open").font(.title2.bold())
            Text(message)
                .font(.footnote.monospaced())
                .multilineTextAlignment(.center)
                .foregroundStyle(.secondary)
            Button("Reset card", role: .destructive) { controller.resetCard() }
                .buttonStyle(.borderedProminent)
                .padding(.top, 8)
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var gear: some View {
        Button { showsDevSheet = true } label: {
            Image(systemName: "gearshape.fill").font(.title3).padding(10)
        }
        .tint(.secondary)
    }

    /// Present while an optional is set, and clear it on dismissal.
    private func binding<Value>(for path: ReferenceWritableKeyPath<HostController, Value?>) -> Binding<Bool> {
        Binding(
            get: { controller[keyPath: path] != nil },
            set: { if !$0 { controller[keyPath: path] = nil } })
    }
}

/// The panel itself: the host's own `UIImageView`, so a frame never travels through SwiftUI.
struct Panel: UIViewRepresentable {
    let controller: HostController

    func makeUIView(context: Context) -> UIImageView { controller.canvas }

    func updateUIView(_ view: UIImageView, context: Context) {}
}
