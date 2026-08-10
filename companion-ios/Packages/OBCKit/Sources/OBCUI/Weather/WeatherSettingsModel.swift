import Foundation
import Observation
import OBCDomain
import OBCTransport
import OBCWeather

/// State for the Weather screen (WX13, epic #1185 phase D): the device's refresh interval, this
/// phone's standing-watch switch, how the last deliveries went, and what the weather service is
/// currently publishing.
///
/// Three rules shape everything here.
///
/// **Device settings live on the device.** The refresh interval is the OBC's own setting: it lives
/// in the Config blob, the device schedules from it, and the OBC's Weather screen is its editor.
/// This app *reports* it and offers no way to change it — a second editor for one value is two
/// places to look when they disagree, and the phone is the one that would be wrong. So there is no
/// write path here at all, and no phone-side mirror either: not connected, the row does not claim a
/// value rather than showing a remembered one. (§7.3 still makes the byte writable over BLE; the
/// contract is frozen and stays as it is. Nothing in this app uses that direction, by choice.)
///
/// **Provenance comes from the manifest.** Not one provider name is written down in the app. The
/// credit lines, tiers, source-run times and staleness deadlines are read from
/// ``WeatherServiceStatusProviding``, so a source added by a baker deploy appears here without an
/// app release — the epic's law, kept by having nothing else to render.
///
/// **The phone cannot ask for weather.** The device raises requests; the phone answers them. So
/// there is no "refresh weather now" button, only *Retry now*, and only when a job is actually
/// owed — a button that promised more would be lying about who is in charge.
@MainActor @Observable
public final class WeatherSettingsModel {
    // MARK: Observable state

    public private(set) var connection: ConnectionState = .connecting
    /// `nil` until `deviceInfo()` lands — "unknown", never "unsupported".
    public private(set) var deviceSupportsWeather: Bool?
    /// The device's stored interval as this build can read it; `nil` when the device named one this
    /// app version does not know (§11.8's tolerant read direction) or nothing has been read yet.
    ///
    /// **Read-only.** The interval is a *device* setting and the OBC's own Weather screen is its
    /// editor; the app reports it and nothing more. There is no phone-side write path to get wrong.
    public private(set) var refresh: WeatherRefresh?
    /// True when the device stored a refresh byte this build cannot name — a newer firmware, not a
    /// broken one. The row says so instead of showing a made-up interval.
    public private(set) var refreshIsUnknownToThisBuild = false
    public private(set) var hasReadConfig = false
    /// The rider's standing-watch preference (phone-side).
    public private(set) var watchEnabled: Bool
    public private(set) var history: [WeatherJobHistoryEntry] = []
    public private(set) var pending: WeatherJobPending?
    public private(set) var service: ServiceState = .loading
    /// A *Retry now* is in flight.
    public private(set) var isRetrying = false

    public enum ServiceState: Equatable, Sendable {
        case loading
        case available(WeatherServiceStatus)
        /// The manifest could not be read. An outage of the rain-map service only — the hourly
        /// forecast comes from MET directly and is unaffected.
        case unavailable
    }

    // MARK: Wiring

    private let transport: any DeviceTransport
    private let historyStore: any WeatherJobHistoryStore
    private let jobs: (any WeatherJobControlling)?
    private let statusProvider: (any WeatherServiceStatusProviding)?
    private let preferences: any WeatherPreferencesStore
    private let now: @Sendable () -> Date
    @ObservationIgnored private var started = false
    @ObservationIgnored private var streamTasks: [Task<Void, Never>] = []

    public init(
        transport: any DeviceTransport,
        historyStore: any WeatherJobHistoryStore,
        jobs: (any WeatherJobControlling)? = nil,
        statusProvider: (any WeatherServiceStatusProviding)? = nil,
        preferences: any WeatherPreferencesStore = InMemoryWeatherPreferencesStore(),
        now: @escaping @Sendable () -> Date = Date.init
    ) {
        self.transport = transport
        self.historyStore = historyStore
        self.jobs = jobs
        self.statusProvider = statusProvider
        self.preferences = preferences
        self.watchEnabled = preferences.loadWeatherWatchEnabled()
        self.now = now
    }

    deinit {
        streamTasks.forEach { $0.cancel() }
    }

    /// Subscribe the link state and load everything the screen shows (call once, from `.task`).
    /// The stream loop follows the house rule: `[weak self]` + `guard let self` inside the loop.
    public func start() {
        guard !started else { return }
        started = true
        streamTasks.append(Task { [weak self, transport] in
            for await state in transport.state {
                guard let self else { return }
                let wasConnected = connection == .connected
                connection = state
                // A link that just came up is the first chance to learn the device's truth.
                if state == .connected, !wasConnected { await readDevice() }
            }
        })
        Task { [weak self] in await self?.readDevice() }
        Task { [weak self] in await self?.reloadJobState() }
        Task { [weak self] in await self?.loadServiceStatus() }
    }

    /// The view's `.task` hook. The first appearance starts the streams; a later one re-reads,
    /// because everything this screen shows is now somebody else's to change — the interval on the
    /// device, the ring in the background job, the manifest on the server — and a rider coming back
    /// from the OBC's own Weather screen expects the interval they just set to be here.
    public func appeared() async {
        guard started else {
            start()
            return
        }
        await refreshAll()
    }

    /// Re-read everything that can change behind the screen's back (the pull-to-refresh / reappear
    /// hook). Deliberately cheap: the manifest read rides the client's 60 s cache.
    public func refreshAll() async {
        await readDevice()
        await reloadJobState()
        await loadServiceStatus()
    }

    // MARK: Device truth

    private func readDevice() async {
        if let info = try? await transport.deviceInfo() {
            deviceSupportsWeather = info.supportsWeather
        }
        guard let config = try? await transport.readConfig() else { return }
        applyReadConfig(config)
    }

    private func applyReadConfig(_ config: DeviceConfig) {
        hasReadConfig = true
        // `effectiveWeatherRefresh` is the one correct reader: an absent field is the device's
        // documented default (30 min), and only an interval this build cannot name reads as `nil`.
        refresh = config.effectiveWeatherRefresh
        refreshIsUnknownToThisBuild = config.effectiveWeatherRefresh == nil
    }

    /// Whether the interval is worth stating at all: a device that is there and has weather. Not
    /// connected, the row simply does not claim a value — the interval lives on the OBC, and a
    /// remembered one would be this screen guessing at a setting it does not own.
    public var canStateRefresh: Bool {
        connection == .connected && deviceSupportsWeather != false && hasReadConfig
    }

    // MARK: The standing watch (phone-side)

    /// Arm or disarm the background scan for the device's weather requests. Persisted, and pushed
    /// to the transport immediately — the switch is the setting, not a request to re-launch.
    public func setWatchEnabled(_ enabled: Bool) {
        guard enabled != watchEnabled else { return }
        watchEnabled = enabled
        preferences.saveWeatherWatchEnabled(enabled)
        transport.setWeatherWatch(enabled)
    }

    // MARK: Job state

    private func reloadJobState() async {
        history = await Self.loadHistory(historyStore)
        pending = await jobs?.pendingJob()
    }

    private nonisolated static func loadHistory(
        _ store: any WeatherJobHistoryStore
    ) async -> [WeatherJobHistoryEntry] {
        // The ring is a small file read; keep it off the main actor anyway — a screen must never
        // wait on storage to draw.
        await Task.detached(priority: .utility) { store.entries() }.value
    }

    /// Finish the owed job now. Does nothing when nothing is owed — which is why the view only
    /// offers it then.
    public func retryNow() async {
        guard let jobs, pending != nil, !isRetrying else { return }
        isRetrying = true
        await jobs.retryNow()
        await reloadJobState()
        isRetrying = false
    }

    // MARK: Service status

    private func loadServiceStatus() async {
        guard let statusProvider else {
            service = .unavailable
            return
        }
        do {
            service = .available(try await statusProvider.serviceStatus(now: now()))
        } catch {
            service = .unavailable
        }
    }

    // MARK: Derived copy

    /// The clock the screen renders against — the injected one, so a test can pin every relative
    /// time the view shows without reaching into the formatter.
    public var clockNow: Date { now() }

    /// The most recent job that reached the device, if any.
    public var lastDelivery: WeatherJobHistoryEntry? {
        history.last { $0.outcome == .committed }
    }

    /// The most recent job of any outcome.
    public var lastAttempt: WeatherJobHistoryEntry? { history.last }

    /// The headline status line — one sentence, in the order a rider cares about: something is
    /// happening now, something failed, something arrived, or nothing has yet.
    public var statusLine: String {
        if let pending {
            return WeatherCopy.pendingLine(pending, now: now())
        }
        if let last = lastAttempt, last.outcome == .failed {
            return "Last try failed · \(WeatherCopy.failureLabel(last.failureReason))"
        }
        if let delivered = lastDelivery {
            return "Delivered \(WeatherCopy.relative(delivered.finishedAt, now: now()))"
        }
        return "No weather sent yet"
    }

    /// Whether the schedule row belongs on the screen at all. A firmware without weather has no
    /// schedule to report — its Config may well still carry a refresh byte, but reporting one for a
    /// device that schedules nothing is the same theatre as offering to set it. The banner and the
    /// status footer already say what is going on.
    public var showsRefreshRow: Bool { deviceSupportsWeather != false }

    /// The value column of the read-only "Asks for weather" row — the device's own schedule,
    /// reported. Reads with its label as one sentence: *Asks for weather · Every 30 min.*
    public var refreshValue: String {
        WeatherCopy.refreshValue(
            refresh,
            unknownToThisBuild: refreshIsUnknownToThisBuild,
            connected: connection == .connected,
            hasRead: hasReadConfig)
    }

    /// The value column of the "Last delivered" row.
    public var lastDeliveryValue: String {
        guard let delivered = lastDelivery else { return "Never" }
        return WeatherCopy.relative(delivered.finishedAt, now: now())
    }

    /// Whether a *Retry now* row is worth showing at all.
    public var canRetry: Bool { jobs != nil && pending != nil }

    /// Whether the live-status row is worth a line of its own. A successful last delivery is
    /// already stated by the row above it, and repeating it there ("Last delivered 14 min ago" /
    /// "Delivered 14 min ago") is the kind of filler the copy rules forbid. It earns its place only
    /// when something is in flight or something went wrong.
    public var showsStatusRow: Bool {
        pending != nil || lastAttempt?.outcome == .failed
    }

    /// The status row's label: what is happening now, or what happened last.
    public var statusRowLabel: String { pending != nil ? "Now" : "Last try" }

    /// The plain sentence under the status group: what is owed, who owes it, and — since the
    /// interval row above is read-only — where the one setting on it is actually changed.
    public var statusFooter: String {
        if deviceSupportsWeather == false {
            return "This OBC's firmware doesn't include weather, so it never asks for any."
        }
        if pending != nil {
            return "Your OBC asked for weather. The app fetches it and sends it back over "
                + "Bluetooth — it doesn't need to stay connected in between."
        }
        return "Your OBC asks for weather on its own schedule; the app answers. Change how often "
            + "on the OBC itself, under Weather."
    }

    /// The service group's value line: how old the published state of the world is.
    public var serviceValue: String {
        switch service {
        case .loading: "Checking…"
        case .unavailable: "Unavailable"
        case .available(let status): "Published \(WeatherCopy.relative(status.generatedAt, now: now()))"
        }
    }

    /// The products the manifest currently publishes, newest-published first — the rows the service
    /// group renders. Empty unless a manifest was read.
    public var products: [WeatherServiceProductStatus] {
        guard case .available(let status) = service else { return [] }
        return status.products.sorted {
            ($0.tier.rawValue, $0.id) < ($1.tier.rawValue, $1.id)
        }
    }

    /// Every credit that must be shown, manifest-sourced, plus MET's for the hourly forecast.
    ///
    /// MET's line is the one attribution that does *not* come from the manifest, and cannot: MET is
    /// called by the phone, not by the baker, so it appears in no manifest entry. It comes from
    /// ``WeatherAttribution/met`` — the provider adapter's own declared credit — which is the same
    /// principle (the credit travels with the source), not a hardcoded provider list.
    public var attributions: [(credit: WeatherAttribution, role: String)] {
        var rows: [(WeatherAttribution, String)] = [(.met, "Hourly forecast")]
        if case .available(let status) = service {
            for credit in status.attributions { rows.append((credit, "Rain map")) }
        }
        return rows
    }

    /// Products past their staleness deadline right now — the honest "stale since …" state.
    public var staleProducts: [WeatherServiceProductStatus] {
        guard case .available(let status) = service else { return [] }
        return status.staleProducts(at: now())
    }

    /// The service group's footer: the outage/staleness truth, stated once.
    public var serviceFooter: String {
        switch service {
        case .loading:
            return "Reading what the \(WeatherCopy.serviceName) is publishing."
        case .unavailable:
            return "Couldn't reach the \(WeatherCopy.serviceName). Hourly forecasts come straight "
                + "from MET Norway and still work; the rain map needs this service."
        case .available(let status):
            let stale = status.staleProducts(at: now())
            if stale.isEmpty {
                return "The \(WeatherCopy.serviceName) builds rain maps once, server-side, and "
                    + "publishes them as plain files. Your OBC gets whichever product covers where "
                    + "you are."
            }
            let names = stale.map(\.id).sorted().joined(separator: ", ")
            let since = stale.map(\.stalenessDeadline).min().map {
                WeatherCopy.absolute($0)
            } ?? "—"
            return "Service data stale since \(since) for \(names). Stale rain is never shown as "
                + "dry — your OBC says a weather update is needed instead."
        }
    }
}
