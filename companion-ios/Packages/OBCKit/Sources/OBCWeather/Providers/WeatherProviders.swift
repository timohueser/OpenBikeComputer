import Foundation

/// The worldwide hourly point forecast. One conformer ships (MET Norway), but the protocol exists
/// so replacing the provider is a one-conformer change — the same "formats at the edges" rule the
/// route importers live under.
public protocol HourlyForecastProvider: Sendable {
    /// 24 consecutive hours for the request's position.
    ///
    /// - Throws: ``WeatherProviderError`` — in particular ``WeatherProviderError/unavailable``,
    ///   which is a *state*, not a crash: the caller keeps whatever it already had.
    func hourlyForecast(for request: WeatherRequest, now: Date) async throws -> HourlyForecast
}

/// The corridor precipitation grid. One conformer ships (the OBC weather service client), and it is
/// deliberately the only thing in the app that knows the service exists.
public protocol PrecipitationGridProvider: Sendable {
    /// The dataset's answer for `corridor`, or the honest reason there is none.
    func precipitation(
        for corridor: WeatherCorridor, now: Date
    ) async throws -> PrecipitationOutcome
}

/// A precipitation lookup either produced frames or produced a reason. Both are results — a missing
/// rain map is a state the rider is shown, never an error that discards the hourly section, and it
/// is not the same thing as a dry map, which is frames full of zeroes.
public enum PrecipitationOutcome: Equatable, Sendable {
    case selected(PrecipitationSelection, WeatherDiagnostics)
    case unavailable(NoRainMapReason, WeatherDiagnostics)

    public var selection: PrecipitationSelection? {
        if case let .selected(selection, _) = self { return selection }
        return nil
    }

    public var diagnostics: WeatherDiagnostics {
        switch self {
        case let .selected(_, diagnostics), let .unavailable(_, diagnostics): diagnostics
        }
    }
}

/// Why a provider could not answer. Deliberately coarse: the app's behaviour differs only between
/// "there is nothing usable" and "the bytes were wrong", and a richer error vocabulary at this
/// boundary would leak provider concepts into the domain.
public enum WeatherProviderError: Error, Equatable, Sendable {
    /// No position to ask about — the device had no usable GPS fix.
    case noPosition
    /// Network or provider failure, with no usable cache to fall back on.
    case unavailable
    /// The provider answered, but the payload violated its own contract.
    case malformedResponse
    /// The provider asked us to slow down. Honoured, never retried through.
    case rateLimited(retryAfterSeconds: Int?)
}
