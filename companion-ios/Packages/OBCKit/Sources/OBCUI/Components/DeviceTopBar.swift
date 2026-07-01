import SwiftUI
import OBCDomain

/// The top-bar **sync button**'s three states (design "SYNC" frame): idle
/// (download arrow), syncing (amber spinner — pair with a "3 of 5 rides" line),
/// done (forest check for ~2s, then back to idle — the *consumer* owns that
/// timing).
public enum OBCSyncButtonState: Equatable, Sendable {
    case idle
    case syncing
    case done
}

/// **Device Top Bar** (§9, NEW) — name + battery + sync + settings gear. The
/// only place connection lives: when the link is down the dot loses its glow,
/// the name and battery dim, and sync disables (S4 "degrade, don't block").
/// The gear is the single route into Settings (§2).
public struct DeviceTopBar: View {
    let deviceName: String
    let connection: ConnectionState
    /// Battery percent 0–100, `nil` when unknown (shows "—").
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
        HStack(spacing: 12) {
            HStack(spacing: 9) {
                Circle()
                    .fill(isLinked ? OBCTheme.forest : OBCTheme.inkFaint)
                    .frame(width: 9, height: 9)
                    .background(
                        Circle()
                            .fill(OBCTheme.forest.opacity(isLinked ? 0.18 : 0))
                            .frame(width: 15, height: 15)
                    )
                Text(deviceName)
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(isLinked ? OBCTheme.ink : OBCTheme.inkFaint)
                    .lineLimit(1)
            }
            .accessibilityElement(children: .combine)

            Spacer(minLength: 0)

            HStack(spacing: 8) {
                OBCBatteryIndicator(percent: isLinked ? batteryPercent : nil)
                    .opacity(isLinked ? 1 : 0.5)

                OBCIconButton(disabled: !isLinked) {
                    onSync()
                } label: {
                    syncIcon
                }
                .accessibilityLabel(syncAccessibilityLabel)

                OBCIconButton {
                    onSettings()
                } label: {
                    Image(systemName: "gearshape")
                        .font(.system(size: 17, weight: .medium))
                }
                .accessibilityLabel("Settings")
            }
        }
        .padding(.horizontal, 18)
        .padding(.top, 8)
        .padding(.bottom, 12)
    }

    @ViewBuilder
    private var syncIcon: some View {
        switch syncState {
        case .idle:
            Image(systemName: "arrow.down.to.line")
                .font(.system(size: 16, weight: .medium))
        case .syncing:
            OBCSpinner(color: OBCTheme.amber)
        case .done:
            Image(systemName: "checkmark")
                .font(.system(size: 16, weight: .bold))
                .foregroundStyle(OBCTheme.forest)
        }
    }

    private var syncAccessibilityLabel: String {
        switch syncState {
        case .idle: "Sync tracked rides"
        case .syncing: "Syncing"
        case .done: "Synced"
        }
    }
}

/// The 24×12 battery glyph + mono percent from the design top bar.
public struct OBCBatteryIndicator: View {
    let percent: Int?

    public init(percent: Int?) { self.percent = percent }

    public var body: some View {
        HStack(spacing: 5) {
            ZStack(alignment: .leading) {
                RoundedRectangle(cornerRadius: 3)
                    .strokeBorder(OBCTheme.inkSoft, lineWidth: 1.5)
                if let percent {
                    RoundedRectangle(cornerRadius: 1)
                        .fill(fillColor(for: percent))
                        .padding(3)
                        .frame(width: 3 + 18 * CGFloat(max(0, min(percent, 100))) / 100)
                }
            }
            .frame(width: 24, height: 12)
            .overlay(alignment: .trailing) {
                // The battery nub.
                RoundedRectangle(cornerRadius: 1)
                    .fill(OBCTheme.inkSoft)
                    .frame(width: 2.5, height: 5)
                    .offset(x: 4)
            }

            Text(percent.map { "\($0)%" } ?? "—")
                .font(.obcMono(size: 12, weight: .semibold))
                .foregroundStyle(OBCTheme.inkSoft)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(percent.map { "Battery \($0) percent" } ?? "Battery unknown")
    }

    private func fillColor(for percent: Int) -> Color {
        percent <= 20 ? OBCTheme.warning : OBCTheme.forest
    }
}

/// The 38pt circular **icon button** in the device cluster (`.icon-btn`);
/// 34pt `compact` for large-title trailing actions (`.lg-btn`).
public struct OBCIconButton<Label: View>: View {
    var compact = false
    var disabled = false
    let action: () -> Void
    @ViewBuilder let label: Label

    public init(
        compact: Bool = false,
        disabled: Bool = false,
        action: @escaping () -> Void,
        @ViewBuilder label: () -> Label
    ) {
        self.compact = compact
        self.disabled = disabled
        self.action = action
        self.label = label()
    }

    public var body: some View {
        Button(action: action) {
            label
                .foregroundStyle(OBCTheme.tint)
                .frame(width: compact ? 34 : 38, height: compact ? 34 : 38)
                .background(OBCTheme.panel)
                .clipShape(Circle())
                .overlay(Circle().strokeBorder(OBCTheme.line))
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.5 : 1)
    }
}

/// The design's 20pt ring spinner (`.spinner`) — a stroked arc rotating at
/// 0.8s/turn, colored track + bright cap.
public struct OBCSpinner: View {
    var color: Color = OBCTheme.tint

    public init(color: Color = OBCTheme.tint) { self.color = color }

    public var body: some View {
        TimelineView(.animation) { context in
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
    .background(OBCTheme.parchment)
}
