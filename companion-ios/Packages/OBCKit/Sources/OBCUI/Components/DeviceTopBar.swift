import SwiftUI
import OBCDomain

/// The top-bar sync button's three states. `syncing` pairs with a "3 of 5 rides"
/// line; `done` shows a check until the consumer sets it back to idle, and that
/// timing is the consumer's.
public enum OBCSyncButtonState: Equatable, Sendable {
    case idle
    case syncing
    case done
}

/// The device band: the device's own rust title bar, from the top edge down under the device
/// row. The name is in the device's font; the link state, the battery, sync and the settings
/// gear sit on the right. The band is the only place connection shows: when the link is down,
/// the battery gives way to the state in words and sync disables. The gear is the single route
/// into Settings. Put it at the top of a screen; it draws under the status bar.
public struct DeviceTopBar: View {
    let deviceName: String
    let connection: ConnectionState
    /// Battery percent 0–100, `nil` when unknown (shows "--").
    let batteryPercent: Int?
    let syncState: OBCSyncButtonState
    let onSync: () -> Void
    let onSettings: () -> Void

    public init(
        deviceName: String,
        connection: ConnectionState,
        batteryPercent: Int?,
        syncState: OBCSyncButtonState = .idle,
        onSync: @escaping () -> Void = {},
        onSettings: @escaping () -> Void = {}
    ) {
        self.deviceName = deviceName
        self.connection = connection
        self.batteryPercent = batteryPercent
        self.syncState = syncState
        self.onSync = onSync
        self.onSettings = onSettings
    }

    private var isLinked: Bool { connection == .connected }

    public var body: some View {
        HStack(spacing: 0) {
            name
                .padding(.trailing, 8)
                // One VoiceOver stop for the whole device state; the battery and link text
                // beside the buttons are hidden, because this label says them.
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(accessibilityDescription)
                .accessibilityAddTraits(.isHeader)
                .accessibilityIdentifier("topbar.device")
            Spacer(minLength: 0)
            status
                .padding(.trailing, 4)
                .accessibilityHidden(true)
            bandButton(disabled: !isLinked, action: onSync) { syncIcon }
                .accessibilityLabel(syncAccessibilityLabel)
                .accessibilityIdentifier("topbar.sync")
            bandButton(action: onSettings) {
                Image(systemName: "gearshape")
                    .font(.system(.body, weight: .medium))
            }
            .accessibilityLabel("Settings")
            .accessibilityIdentifier("topbar.settings")
        }
        .padding(.leading, 20)
        .padding(.trailing, 6)
        .padding(.vertical, 3)
        .foregroundStyle(OBCTheme.onRust)
        .background(OBCTheme.rust.ignoresSafeArea(edges: .top))
    }

    /// The name as the device's title bar writes it, a size smaller when it is long.
    private var name: some View {
        let text = deviceName.uppercased()
        return ViewThatFits(in: .horizontal) {
            PixelText(text, size: .label, color: OBCTheme.onRust)
            PixelText(text, size: .caption, color: OBCTheme.onRust)
            PixelText(text, size: .caption, color: OBCTheme.onRust)
                .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
                .clipped()
        }
    }

    /// The battery while linked; otherwise the link state in words.
    @ViewBuilder
    private var status: some View {
        if isLinked {
            OBCBatteryIndicator(percent: batteryPercent)
        } else {
            Text(connectionWords)
                .font(.system(.footnote, weight: .semibold))
                .lineLimit(1)
                .obcFixedGeometryType()
        }
    }

    private var connectionWords: String {
        switch connection {
        case .connected: "Connected"
        case .connecting: "Connecting…"
        case .outOfRange: "Out of range"
        case .disconnected: "Not connected"
        }
    }

    private var accessibilityDescription: String {
        let battery = isLinked
            ? batteryPercent.map { ", battery \($0) percent" } ?? ", battery unknown"
            : ""
        return "\(deviceName), \(connectionWords.lowercased())\(battery)"
    }

    /// A 44pt glyph button on the band.
    private func bandButton<Label: View>(
        disabled: Bool = false,
        action: @escaping () -> Void,
        @ViewBuilder label: () -> Label
    ) -> some View {
        Button(action: action) {
            label()
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
                .obcFixedGeometryType()
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.45 : 1)
    }

    @ViewBuilder
    private var syncIcon: some View {
        switch syncState {
        case .idle:
            Image(systemName: "arrow.triangle.2.circlepath")
                .font(.system(.body, weight: .medium))
        case .syncing:
            OBCSpinner(color: OBCTheme.onRust)
        case .done:
            Image(systemName: "checkmark")
                .font(.system(.body, weight: .bold))
        }
    }

    private var syncAccessibilityLabel: String {
        switch syncState {
        case .idle: "Sync rides"
        case .syncing: "Syncing"
        case .done: "Synced"
        }
    }
}

/// The battery on the band: a drawn cell and the percent in the device's font. At 20 % or less
/// the charge turns amber.
public struct OBCBatteryIndicator: View {
    let percent: Int?

    public init(percent: Int?) { self.percent = percent }

    public var body: some View {
        HStack(spacing: 6) {
            ZStack(alignment: .leading) {
                RoundedRectangle(cornerRadius: 3)
                    .strokeBorder(OBCTheme.onRust, lineWidth: 1.5)
                if let percent {
                    RoundedRectangle(cornerRadius: 1)
                        .fill(percent <= 20 ? OBCTheme.amber : OBCTheme.onRust)
                        .padding(3)
                        .frame(width: 6 + 16 * CGFloat(max(0, min(percent, 100))) / 100)
                }
            }
            .frame(width: 22, height: 12)
            .overlay(alignment: .trailing) {
                // The battery nub.
                RoundedRectangle(cornerRadius: 1)
                    .fill(OBCTheme.onRust)
                    .frame(width: 2.5, height: 5)
                    .offset(x: 3.5)
            }
            .padding(.trailing, 3)

            PixelText(percent.map { "\($0)%" } ?? "--", color: OBCTheme.onRust)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(percent.map { "Battery \($0) percent" } ?? "Battery unknown")
    }
}

/// A 20pt ring spinner: a stroked arc rotating once per 0.8 s. With Reduce Motion the arc holds
/// still.
public struct OBCSpinner: View {
    var color: Color = OBCTheme.tint

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    public init(color: Color = OBCTheme.tint) { self.color = color }

    public var body: some View {
        TimelineView(.animation(paused: reduceMotion)) { context in
            let phase = context.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 0.8) / 0.8
            ZStack {
                Circle().strokeBorder(color.opacity(0.25), lineWidth: 2.5)
                Circle()
                    .trim(from: 0, to: 0.25)
                    .stroke(color, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                    .padding(1.25)
                    .rotationEffect(.radians(2 * .pi * phase))
            }
        }
        .frame(width: 20, height: 20)
    }
}

#Preview("Device top bar") {
    VStack(spacing: 0) {
        DeviceTopBar(deviceName: "Trailhead", connection: .connected, batteryPercent: 82)
        DeviceTopBar(deviceName: "Trailhead", connection: .connected, batteryPercent: 82, syncState: .syncing)
        DeviceTopBar(deviceName: "Trailhead", connection: .connected, batteryPercent: 12, syncState: .done)
        DeviceTopBar(deviceName: "Trailhead", connection: .outOfRange, batteryPercent: 82)
    }
    .background(OBCTheme.page)
}
