import SwiftUI
import OBCDomain
import OBCTransport
import OBCUI

/// Placeholder root. Proves the transport seam end to end: it holds only
/// `any DeviceTransport`, fetches once on appear, and renders the result. The
/// real screen stack (main list, route detail, sync…) lands in B3+.
struct RootView: View {
    let transport: any DeviceTransport

    @State private var info: DeviceInfo?
    @State private var didFail = false

    var body: some View {
        VStack(spacing: 10) {
            Text("OBC")
                .font(.system(size: 44, weight: .bold, design: .rounded))
                .foregroundStyle(OBCTheme.tint)

            if let info {
                Text(info.name)
                    .font(.headline)
                    .foregroundStyle(OBCTheme.ink)
                Text(info.firmwareVersion)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
            } else if didFail {
                Text("No transport")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            } else {
                ProgressView()
            }

            Text("companion scaffold · B0")
                .font(.caption2)
                .foregroundStyle(.secondary)
                .accessibilityIdentifier("scaffoldTag")
        }
        .padding()
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(OBCTheme.parchment)
        .task {
            do {
                info = try await transport.deviceInfo()
            } catch {
                didFail = true
            }
        }
    }
}

#if DEBUG
import OBCMock

#Preview {
    RootView(transport: MockTransport())
}
#endif
