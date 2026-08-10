import SwiftUI
import OBCWeather

/// The Weather screen (WX13) — pushed from Settings.
///
/// It deliberately does **not** duplicate the device's forecast UI: the OBC shows the weather, this
/// shows how the weather gets there. Three groups in the order a rider needs them — what this phone
/// is allowed to do in the background, how the last runs went (including the device's own schedule,
/// reported), and what the service is publishing — then the two pushed pages that would drown the
/// list: the ring, and the data-flow story.
///
/// **One editable thing on this screen**, and it is the phone's: the standing background watch.
/// The refresh interval is the *device's* setting and appears here as a status line only — the OBC's
/// own Weather screen is where it is changed. A second editor for one value is two places to look
/// when they disagree, and this one would be the one that is wrong.
public struct WeatherSettingsView: View {
    // Not `@Bindable`: with the interval editor gone there is no `$model` binding left to make —
    // the one control on the screen drives the model through a method, not a projected value.
    private let model: WeatherSettingsModel
    private let onOpenDiagnostics: () -> Void
    private let onOpenPrivacy: () -> Void
    @Environment(\.openURL) private var openURL

    public init(
        model: WeatherSettingsModel,
        onOpenDiagnostics: @escaping () -> Void,
        onOpenPrivacy: @escaping () -> Void
    ) {
        self.model = model
        self.onOpenDiagnostics = onOpenDiagnostics
        self.onOpenPrivacy = onOpenPrivacy
    }

    public var body: some View {
        ScrollView {
            VStack(spacing: 26) {
                if model.deviceSupportsWeather == false { unsupportedBanner }
                backgroundGroup
                statusGroup
                serviceGroup
                aboutGroup
            }
            .padding(.horizontal, 20)
            .padding(.top, 18)
            .padding(.bottom, 30)
        }
        .background(OBCTheme.parchment.ignoresSafeArea())
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("weather.screen")
        .navigationTitle("Weather")
        #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
        #endif
        .task { await model.appeared() }
    }

    // MARK: Capability

    /// A device whose firmware predates weather. Stated once, at the top, instead of letting a
    /// screen full of blanks imply it.
    private var unsupportedBanner: some View {
        OBCGroupedSection(footer: "Weather arrived in a later firmware. Everything below describes "
            + "the service; nothing reaches this OBC until it's updated.") {
            OBCListRow(
                icon: "exclamationmark.triangle",
                iconColor: OBCTheme.amber,
                label: "This OBC has no weather support",
                showsDivider: false
            )
        }
    }

    // MARK: Background weather (this phone)

    private var backgroundGroup: some View {
        OBCGroupedSection("On this phone", footer: backgroundFooter) {
            OBCListRow(
                icon: "antenna.radiowaves.left.and.right",
                iconColor: OBCTheme.forest,
                label: "Background weather",
                showsDivider: false
            ) {
                Toggle(
                    "Background weather",
                    isOn: Binding(
                        get: { model.watchEnabled },
                        set: { model.setWatchEnabled($0) }
                    )
                )
                .labelsHidden()
                .tint(OBCTheme.forest)
            }
            .accessibilityIdentifier("weather.backgroundWatch")
        }
    }

    private var backgroundFooter: String {
        (model.watchEnabled
            ? "The app listens for your OBC's weather requests while it's in the background. It "
                + "costs a little battery; switching it off means weather only updates while the "
                + "app is open."
            : "Off: weather only updates while the app is open. Your OBC keeps asking; nothing "
                + "answers until you open the app.")
            + " iOS decides when a background app may run, so timing is best effort — and "
            + "force-quitting the app stops it until you open it again."
    }

    // MARK: Status

    private var statusGroup: some View {
        OBCGroupedSection("Status", footer: model.statusFooter) {
            // The device's own schedule, **reported**. It sits here rather than in a group of its
            // own because that is what it is now — a status line, one of the facts about how
            // weather is reaching this OBC, not a control. It reads with its value as one
            // sentence: "Asks for weather · Every 30 min".
            if model.showsRefreshRow {
                OBCListRow(
                    icon: "clock.arrow.circlepath",
                    iconColor: OBCTheme.water,
                    label: "Asks for weather",
                    value: model.refreshValue
                )
                .accessibilityIdentifier("weather.refresh")
            }
            OBCListRow(
                icon: "checkmark.seal",
                iconColor: OBCTheme.forest,
                label: "Last delivered",
                value: model.lastDeliveryValue,
                showsDivider: model.showsStatusRow
            )
            if model.showsStatusRow {
                OBCListRow(
                    icon: "arrow.triangle.2.circlepath",
                    iconColor: OBCTheme.wood,
                    label: model.statusRowLabel,
                    value: model.statusLine,
                    showsDivider: model.canRetry
                )
                .accessibilityIdentifier("weather.status")
            }
            if model.canRetry {
                OBCListRow(
                    icon: "arrow.clockwise",
                    iconColor: OBCTheme.water,
                    label: model.isRetrying ? "Retrying…" : "Retry now",
                    disabled: model.isRetrying,
                    showsDivider: false,
                    action: { Task { await model.retryNow() } }
                )
                .accessibilityIdentifier("weather.retry")
            }
        }
    }

    // MARK: Weather service (manifest-sourced)

    private var serviceGroup: some View {
        OBCGroupedSection("Weather service", footer: model.serviceFooter) {
            OBCListRow(
                icon: "cloud",
                iconColor: OBCTheme.water,
                label: "Service data",
                value: model.serviceValue,
                showsDivider: !model.products.isEmpty || !model.attributions.isEmpty
            )
            .accessibilityIdentifier("weather.serviceData")
            ForEach(Array(model.products.enumerated()), id: \.element.id) { index, product in
                productRow(product, isLast: index == model.products.count - 1)
            }
            ForEach(Array(model.attributions.enumerated()), id: \.offset) { index, row in
                attributionRow(
                    row.credit, role: row.role, isLast: index == model.attributions.count - 1)
            }
        }
    }

    private func productRow(_ product: WeatherServiceProductStatus, isLast: Bool) -> some View {
        HStack(spacing: 12) {
            OBCIconTile(
                systemImage: product.isFresh(at: model.clockNow) ? "dot.radiowaves.up.forward" : "clock.badge.exclamationmark",
                color: product.isFresh(at: model.clockNow) ? OBCTheme.wood : OBCTheme.amber)
            VStack(alignment: .leading, spacing: 2) {
                Text(product.id)
                    .font(.obcMono(size: 13))
                    .foregroundStyle(OBCTheme.ink)
                Text(WeatherCopy.productSummary(product))
                    .font(.system(size: 12.5))
                    .foregroundStyle(OBCTheme.inkFaint)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Text(WeatherCopy.productFreshness(product, now: model.clockNow))
                .font(.system(size: 12.5))
                .multilineTextAlignment(.trailing)
                .foregroundStyle(product.isFresh(at: model.clockNow) ? OBCTheme.inkFaint : OBCTheme.warning)
        }
        .padding(.vertical, 12)
        .padding(.horizontal, 16)
        .frame(minHeight: 52)
        .overlay(alignment: .bottom) {
            if !isLast || !model.attributions.isEmpty {
                OBCTheme.screenLine.frame(height: 1).padding(.leading, 56)
            }
        }
    }

    /// One credit line, straight from the source that requires it. Tapping opens the licence.
    private func attributionRow(
        _ credit: WeatherAttribution, role: String, isLast: Bool
    ) -> some View {
        Button {
            if let url = URL(string: credit.url) { openURL(url) }
        } label: {
            HStack(spacing: 12) {
                OBCIconTile(
                    systemImage: "text.badge.checkmark", color: OBCTheme.parchment3,
                    glyphColor: OBCTheme.inkSoft)
                VStack(alignment: .leading, spacing: 2) {
                    Text(credit.text)
                        .font(.system(size: 15))
                        .foregroundStyle(OBCTheme.ink)
                        .fixedSize(horizontal: false, vertical: true)
                        .multilineTextAlignment(.leading)
                    Text(role)
                        .font(.system(size: 12.5))
                        .foregroundStyle(OBCTheme.inkFaint)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Image(systemName: "arrow.up.right")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(OBCTheme.inkFaint)
            }
            .padding(.vertical, 12)
            .padding(.horizontal, 16)
            .frame(minHeight: 52)
            .contentShape(Rectangle())
            .overlay(alignment: .bottom) {
                if !isLast {
                    OBCTheme.screenLine.frame(height: 1).padding(.leading, 56)
                }
            }
        }
        .buttonStyle(.plain)
    }

    // MARK: The two pushed pages

    private var aboutGroup: some View {
        OBCGroupedSection(footer: WeatherCopy.aboutFooter) {
            OBCListRow(
                icon: "list.bullet.rectangle",
                iconColor: OBCTheme.wood,
                label: "Sync diagnostics",
                showsChevron: true,
                action: onOpenDiagnostics
            )
            .accessibilityIdentifier("weather.diagnosticsLink")
            OBCListRow(
                icon: "hand.raised",
                iconColor: OBCTheme.forest,
                label: "How weather reaches your OBC",
                showsChevron: true,
                showsDivider: false,
                action: onOpenPrivacy
            )
            .accessibilityIdentifier("weather.privacyLink")
        }
    }
}
