import SwiftUI

/// One connected service's row state in the sync block.
public struct OBCServiceStatus: Identifiable {
    public enum SyncState {
        /// Forest check + "Uploaded on import".
        case uploaded(String)
        /// Faint line + a per-ride ghost Upload button.
        case notUploaded(String)
    }

    public let name: String
    public let systemImage: String
    public let tileColor: Color
    public let state: SyncState

    public var id: String { name }

    public init(name: String, systemImage: String, tileColor: Color, state: SyncState) {
        self.name = name
        self.systemImage = systemImage
        self.tileColor = tileColor
        self.state = state
    }
}

/// Connected-services sync block — per-service rows (Strava, Komoot) with a
/// synced-state line and a per-ride Upload when auto-sync is off or a push
/// failed. Shipped coming-soon: the seam is designed now (`comingSoon: true`
/// badges the header); the Settings side pairs it with an auto-sync-on-import
/// toggle.
public struct OBCConnectedServicesBlock: View {
    let services: [OBCServiceStatus]
    var comingSoon: Bool
    let onUpload: (String) -> Void

    public init(
        services: [OBCServiceStatus],
        comingSoon: Bool = true,
        onUpload: @escaping (String) -> Void = { _ in }
    ) {
        self.services = services
        self.comingSoon = comingSoon
        self.onUpload = onUpload
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                OBCEyebrow("Connected services")
                if comingSoon { OBCSoonBadge() }
            }

            VStack(spacing: 0) {
                ForEach(services) { service in
                    row(service, isLast: service.id == services.last?.id)
                }
            }
            .background(OBCTheme.panel)
            .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
            .overlay(
                RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.line)
            )

            Text("Turn on auto-sync on import in Settings → Connected services to push new rides for you. If it's off or a push fails, upload a ride here.")
                .font(.system(size: 12.5))
                .foregroundStyle(OBCTheme.inkFaint)
                .padding(.horizontal, 10)
        }
        .opacity(comingSoon ? 0.75 : 1)
        .disabled(comingSoon)
    }

    private func row(_ service: OBCServiceStatus, isLast: Bool) -> some View {
        HStack(spacing: 12) {
            OBCIconTile(systemImage: service.systemImage, color: service.tileColor)

            VStack(alignment: .leading, spacing: 2) {
                Text(service.name)
                    .font(.system(size: 16, weight: .medium))
                    .foregroundStyle(OBCTheme.ink)
                switch service.state {
                case .uploaded(let line):
                    Label(line, systemImage: "checkmark")
                        .font(.obcMono(size: 12))
                        .foregroundStyle(OBCTheme.forest)
                        .labelStyle(.titleAndIcon)
                case .notUploaded(let line):
                    Text(line)
                        .font(.obcMono(size: 12))
                        .foregroundStyle(OBCTheme.inkFaint)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            if case .notUploaded = service.state {
                Button("Upload") { onUpload(service.name) }
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(OBCTheme.tint)
                    .padding(.vertical, 8)
                    .padding(.horizontal, 14)
                    .overlay(
                        RoundedRectangle(cornerRadius: 10)
                            .strokeBorder(OBCTheme.forest.opacity(0.4), lineWidth: 1.5)
                    )
                    .buttonStyle(.plain)
            }
        }
        .padding(.vertical, 14)
        .padding(.horizontal, 16)
        .frame(minHeight: 52)
        .overlay(alignment: .bottom) {
            if !isLast { OBCTheme.screenLine.frame(height: 1).padding(.leading, 56) }
        }
    }
}

#Preview("Connected services") {
    OBCConnectedServicesBlock(services: [
        OBCServiceStatus(
            name: "Strava",
            systemImage: "bolt.fill",
            tileColor: OBCTheme.coral,
            state: .uploaded("Uploaded on import")
        ),
        OBCServiceStatus(
            name: "Komoot",
            systemImage: "location.circle",
            tileColor: OBCTheme.wood,
            state: .notUploaded("Not uploaded")
        ),
    ])
    .padding(20)
    .background(OBCTheme.parchment)
}
