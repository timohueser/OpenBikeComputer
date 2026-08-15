import Foundation

public enum WeatherAssemblyOutcome: Equatable, Sendable {
    case bundle(BuiltWeatherBundle)
    /// Both provider revisions are at-or-before the held bundle's build time. The phone can satisfy
    /// the request with a seven-byte command instead of recreating and uploading the bundle.
    case unchanged(retryAfterSeconds: UInt16, precipitationGeneration: String)
}

/// Runs one weather job's two halves and merges them into a bundle.
///
/// This is the whole of the "phone owns corridor selection, OBCG → OBCW assembly, the MET hourly
/// fetch" sentence in the epic, and nothing else: no BLE (WX9), no UI (WX13), no scheduling. It
/// exists as its own type because the independence rule needs somewhere to be enforced and tested —
/// the two providers run concurrently, and a precipitation failure is *caught here* so it can never
/// take a perfectly good hourly forecast down with it.
public struct WeatherAssembler: Sendable {
    /// Matches the device's grace beyond a quarter-hour publication boundary.
    static let publicationGrace: TimeInterval = 2 * 60
    static let unchangedRetry: TimeInterval = 60
    private let hourlyProvider: any HourlyForecastProvider
    private let precipitationProvider: any PrecipitationGridProvider
    private let builder: WeatherBundleBuilder

    public init(
        hourlyProvider: any HourlyForecastProvider,
        precipitationProvider: any PrecipitationGridProvider,
        builder: WeatherBundleBuilder = WeatherBundleBuilder()
    ) {
        self.hourlyProvider = hourlyProvider
        self.precipitationProvider = precipitationProvider
        self.builder = builder
    }

    /// - Parameter generation: the monotonic cache generation for the OBCW header. The caller owns
    ///   monotonicity (WX9 knows what the device already holds); passing it in also keeps this
    ///   function a pure function of its inputs, which is what makes the bundle reproducible.
    public func assemble(
        request: WeatherRequest, generation: UInt32, now: Date
    ) async throws -> BuiltWeatherBundle {
        guard let corridor = WeatherCorridor.around(request) else {
            throw WeatherProviderError.noPosition
        }
        async let hourlyTask = hourlyProvider.hourlyForecast(for: request, now: now)
        async let precipitationTask = precipitationProvider.precipitation(
            for: corridor, now: now)

        // The rain half may fail entirely; that is a state, not a job failure.
        let precipitation: PrecipitationOutcome
        do {
            precipitation = try await precipitationTask
        } catch {
            precipitation = .unavailable(.serviceUnavailable, WeatherDiagnostics())
        }
        // The hourly half may not: without 24 hours there is nothing to send, and the device keeps
        // the bundle it already has.
        let hourly = try await hourlyTask

        var reason: NoRainMapReason?
        if case let .unavailable(unavailable, _) = precipitation { reason = unavailable }
        return try builder.build(
            request: request, corridor: corridor, hourly: hourly,
            precipitation: precipitation.selection, noRainMapReason: reason,
            generation: generation, now: now, diagnostics: precipitation.diagnostics)
    }

    /// Probe the mutable provider identities before paying for rain-frame Range reads. A provider
    /// that cannot state a revision, a missing provider timestamp, an hourly-only held bundle, or a
    /// material location change all fail conservative and use the ordinary full build.
    public func assembleIfChanged(
        request: WeatherRequest, generation: UInt32, heldBundleGeneratedAt: Date?,
        allowHeldBundleReuse: Bool, now: Date
    ) async throws -> WeatherAssemblyOutcome {
        guard let heldBundleGeneratedAt, allowHeldBundleReuse else {
            return .bundle(try await assemble(request: request, generation: generation, now: now))
        }

        async let hourlyTask = hourlyProvider.hourlyForecast(for: request, now: now)
        async let revisionTask = precipitationProvider.currentRevision(now: now)
        let hourly = try await hourlyTask
        let revision: PrecipitationRevision?
        do {
            revision = try await revisionTask
        } catch {
            // Rain remains independent from hourly weather. The established full assembler turns
            // a service outage into an explicitly labelled hourly-only bundle; a failed optional
            // optimisation must not turn that valid result into a failed job.
            guard let corridor = WeatherCorridor.around(request) else {
                throw WeatherProviderError.noPosition
            }
            return .bundle(try builder.build(
                request: request, corridor: corridor, hourly: hourly,
                precipitation: nil, noRainMapReason: .serviceUnavailable,
                generation: generation, now: now))
        }
        guard let revision,
              let hourlyUpdatedAt = hourly.providerUpdatedAt,
              revision.generatedAt <= heldBundleGeneratedAt,
              hourlyUpdatedAt <= heldBundleGeneratedAt
        else {
            // `hourly` is now warm in the provider cache, so the ordinary build does not repeat a
            // network body even though keeping this fallback on the public seam makes it simple.
            return .bundle(try await assemble(request: request, generation: generation, now: now))
        }

        let earliest = revision.nextGenerationExpectedAt.addingTimeInterval(Self.publicationGrace)
        let delay = earliest > now ? earliest.timeIntervalSince(now) : Self.unchangedRetry
        return .unchanged(
            retryAfterSeconds: UInt16(min(3_600, max(1, delay.rounded(.up)))),
            precipitationGeneration: revision.generation)
    }
}
