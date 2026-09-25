import SwiftUI
import OBCDomain

/// What the device holds of this route, and the one action when there is something to do: the
/// state row, then an amber Send or Update while the device is connected. An up-to-date copy gets
/// no button, and a device out of reach gets a plain line instead of one. The route page and the
/// trip page both lead with it.
public struct DeviceCopyStatus: View {
    let state: OnDeviceState
    let connection: ConnectionState
    let deviceName: String
    /// The screen's identifier prefix: `detail` on the route page, `trip` on the trip page.
    let idPrefix: String
    let onSend: () -> Void

    @Environment(\.dynamicTypeSize) private var typeSize

    public init(
        state: OnDeviceState, connection: ConnectionState, deviceName: String, idPrefix: String = "detail",
        onSend: @escaping () -> Void
    ) {
        self.state = state
        self.connection = connection
        self.deviceName = deviceName
        self.idPrefix = idPrefix
        self.onSend = onSend
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                // At accessibility sizes the marker sits above its line, so the line keeps the width.
                let layout = typeSize.isAccessibilitySize
                    ? AnyLayout(VStackLayout(alignment: .leading, spacing: 6))
                    : AnyLayout(HStackLayout(spacing: 8))
                layout {
                    marker
                    Text(stateLine)
                        .font(.system(.subheadline))
                        .foregroundStyle(OBCTheme.secondary)
                }
                if let linkLine {
                    Text(linkLine)
                        .font(.system(.subheadline))
                        .foregroundStyle(OBCTheme.secondary)
                }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel([stateLine, linkLine].compactMap { $0 }.joined(separator: ". "))
            .accessibilityIdentifier("\(idPrefix).deviceState")

            if let actionTitle, connection == .connected {
                Button(actionTitle, action: onSend)
                    .buttonStyle(.obcPrimary)
                    .accessibilityIdentifier("\(idPrefix).upload")
            }
        }
    }

    @ViewBuilder
    private var marker: some View {
        switch state {
        case .notOnDevice:
            RoundedRectangle(cornerRadius: 2)
                .strokeBorder(OBCTheme.secondary, lineWidth: 1.6)
                .frame(width: 10, height: 10)
        case .upToDate, .outdated:
            OnDeviceChip(upToDate: state == .upToDate)
        }
    }

    private var stateLine: String {
        switch state {
        case .notOnDevice: "Not on \(deviceName) yet"
        case .upToDate: "Up to date on \(deviceName)"
        case .outdated: "\(deviceName) has an older version"
        }
    }

    private var actionTitle: String? {
        switch state {
        case .notOnDevice: "Send to \(deviceName)"
        case .outdated: "Update on \(deviceName)"
        case .upToDate: nil
        }
    }

    /// Why there is no button when there is something to send. The top bar carries the reconnect.
    private var linkLine: String? {
        guard actionTitle != nil else { return nil }
        return switch connection {
        case .connected: nil
        case .connecting: "Connecting to \(deviceName)…"
        case .outOfRange: "\(deviceName) is out of range. Move closer to send."
        case .disconnected: "\(deviceName) is not connected. Turn it on to send."
        }
    }
}

#Preview("Device copy status") {
    VStack(alignment: .leading, spacing: 24) {
        DeviceCopyStatus(state: .notOnDevice, connection: .connected, deviceName: "Trailhead", onSend: {})
        DeviceCopyStatus(state: .upToDate, connection: .connected, deviceName: "Trailhead", onSend: {})
        DeviceCopyStatus(state: .outdated, connection: .connected, deviceName: "Trailhead", onSend: {})
        DeviceCopyStatus(state: .notOnDevice, connection: .outOfRange, deviceName: "Trailhead", onSend: {})
    }
    .padding(20)
    .background(OBCTheme.page)
}
