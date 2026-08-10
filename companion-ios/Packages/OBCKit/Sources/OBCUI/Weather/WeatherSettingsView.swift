import SwiftUI
import OBCDomain
import OBCTransport
import OBCWeather

/// The Weather screen (WX13) — pushed from Settings.
///
/// It deliberately does **not** duplicate the device's forecast UI: the OBC shows the weather, this
/// shows how the weather gets there. Four groups in the order a rider needs them — how often, what
/// this phone is allowed to do in the background, how the last runs went, and what the service is
/// publishing — then the two pushed pages that would drown the list: the ring, and the data-flow
/// story.
public struct WeatherSettingsView: View {
    @Bindable private var model: WeatherSettingsModel
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
                refreshGroup
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
        .obcToast(
            isPresented: $model.refreshWriteFailed,
            systemImage: "exclamationmark.triangle",
            message: "Couldn't change the interval on your OBC. It kept its current setting.",
            duration: .seconds(4)
        )
        .task { model.start() }
    }

    // MARK: Capability

    /// A device whose firmware predates weather. Stated once, at the top, instead of letting four
    /// dimmed groups imply it.
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

    // MARK: How often (spec §7.3 / §11.8)

    private var refreshGroup: some View {
        OBCGroupedSection("How often", footer: refreshFooter) {
            Menu {
                ForEach(WeatherRefresh.allCases, id: \.self) { option in
                    Button {
                        model.setRefresh(option)
                    } label: {
                        if option == model.refresh {
                            Label(WeatherCopy.refreshLabel(option), systemImage: "checkmark")
                        } else {
                            Text(WeatherCopy.refreshLabel(option))
                        }
                    }
                }
            } label: {
                HStack(spacing: 12) {
                    OBCIconTile(systemImage: "clock.arrow.circlepath", color: OBCTheme.water)
                    Text("Ask for weather")
                        .font(.system(size: 16))
                        .foregroundStyle(model.canEditRefresh ? OBCTheme.ink : OBCTheme.inkFaint)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Text(WeatherCopy.refreshValue(
                        model.refresh,
                        unknownToThisBuild: model.refreshIsUnknownToThisBuild,
                        hasRead: model.hasReadConfig))
                        .font(.system(size: 15))
                        .foregroundStyle(OBCTheme.inkFaint)
                    Image(systemName: "chevron.up.chevron.down")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(OBCTheme.inkFaint)
                }
                .padding(.vertical, 14)
                .padding(.horizontal, 16)
                .frame(minHeight: 52)
                .contentShape(Rectangle())
            }
            .disabled(!model.canEditRefresh)
            .accessibilityIdentifier("weather.refresh")
        }
    }

    private var refreshFooter: String {
        if model.deviceSupportsWeather == false {
            return "The interval is stored on the OBC. This one has no weather to schedule."
        }
        if !model.canEditRefresh {
            return "The interval is stored on your OBC, so it can only be changed while connected."
        }
        if model.refreshIsUnknownToThisBuild {
            return "Your OBC is set to an interval this app version doesn't know — a newer "
                + "firmware. Picking one here replaces it."
        }
        return "Only while you're riding. Opening Weather on the OBC always asks straight away."
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
        OBCGroupedSection(footer: "The weather service never receives your position. Only MET "
            + "Norway does, for the hourly forecast.") {
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
