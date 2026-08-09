import Foundation

/// Runs one weather job's two halves and merges them into a bundle.
///
/// This is the whole of the "phone owns corridor selection, OBCG → OBCW assembly, the MET hourly
/// fetch" sentence in the epic, and nothing else: no BLE (WX9), no UI (WX13), no scheduling. It
/// exists as its own type because the independence rule needs somewhere to be enforced and tested —
/// the two providers run concurrently, and a precipitation failure is *caught here* so it can never
/// take a perfectly good hourly forecast down with it.
public struct WeatherAssembler: Sendable {
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
        guard let corridor = WeatherCorridor.projected(for: request) else {
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
}
