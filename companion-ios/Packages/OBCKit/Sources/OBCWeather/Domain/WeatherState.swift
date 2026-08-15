import Foundation
import OBCWeatherWire

/// One hour of conditions, as the app holds them.
///
/// Every optional is genuinely "the source did not supply this", never a zero standing in for
/// missing data — OBCW §4 forbids normalizing an absent precipitation amount to zero, and the
/// sentinel translation happens once, at the wire edge in ``WeatherBundleBuilder``.
public struct HourlyCondition: Equatable, Sendable {
    /// Beginning of the represented hour; the interval is `[validAt, validAt + 3600)`.
    public var validAt: Date
    public var temperatureCelsius: Double?
    /// Total accumulation during this hour in millimetres.
    public var precipitationMillimetres: Double?
    /// Probability of precipitation during this hour, 0...100.
    public var precipitationProbabilityPercent: Double?
    /// The canonical condition table of `OBCW_Spec.md` §4.1 — the one shared vocabulary, so the
    /// domain does not carry a second enum that could drift from the wire's.
    public var condition: OBCWeatherCondition
    /// Meteorological degrees clockwise from true north (where the wind comes *from*).
    public var windFromDegrees: Double?
    public var windSpeedMetresPerSecond: Double?
    public var windGustMetresPerSecond: Double?

    public init(
        validAt: Date, temperatureCelsius: Double? = nil, precipitationMillimetres: Double? = nil,
        precipitationProbabilityPercent: Double? = nil,
        condition: OBCWeatherCondition = .unavailable, windFromDegrees: Double? = nil,
        windSpeedMetresPerSecond: Double? = nil, windGustMetresPerSecond: Double? = nil
    ) {
        self.validAt = validAt
        self.temperatureCelsius = temperatureCelsius
        self.precipitationMillimetres = precipitationMillimetres
        self.precipitationProbabilityPercent = precipitationProbabilityPercent
        self.condition = condition
        self.windFromDegrees = windFromDegrees
        self.windSpeedMetresPerSecond = windSpeedMetresPerSecond
        self.windGustMetresPerSecond = windGustMetresPerSecond
    }
}

/// Who to credit and where the licence lives. Manifest data for the rain dataset, a WX1 constant for
/// MET; the device never sees either (OBCW carries no strings) — this is the phone's to display.
///
/// `sourceID` is the manifest's own name for the upstream, carried because the dataset is a *mosaic*
/// of several: every source that may have painted a cell must be credited, and there is no per-cell
/// provenance to narrow it to. It is provenance only, never a switch.
public struct WeatherAttribution: Equatable, Sendable, Hashable {
    public var text: String
    public var url: String
    public var sourceID: String?

    public init(text: String, url: String, sourceID: String? = nil) {
        self.text = text
        self.url = url
        self.sourceID = sourceID
    }

    /// The exact credit line the MET licence requires (WX1 decision record).
    public static let met = WeatherAttribution(
        text: "Data from MET Norway", url: "https://docs.api.met.no/doc/License.html")
}

/// A complete hourly forecast for one point, as an ``HourlyForecastProvider`` returns it.
public struct HourlyForecast: Equatable, Sendable {
    /// Exactly 24 consecutive hours; the builder rejects anything else rather than padding.
    public var hours: [HourlyCondition]
    public var attribution: WeatherAttribution
    /// When these bytes were retrieved. Survives into the cache so a cached forecast can be shown
    /// with its true age instead of pretending to be current.
    public var retrievedAt: Date
    /// The provider's own update time (`Last-Modified`), when it stated one.
    public var providerUpdatedAt: Date?
    /// True when this came from the local cache rather than a fresh response.
    public var isFromCache: Bool

    public init(
        hours: [HourlyCondition], attribution: WeatherAttribution, retrievedAt: Date,
        providerUpdatedAt: Date? = nil, isFromCache: Bool = false
    ) {
        self.hours = hours
        self.attribution = attribution
        self.retrievedAt = retrievedAt
        self.providerUpdatedAt = providerUpdatedAt
        self.isFromCache = isFromCache
    }
}

/// What a frame's bytes mean. Mirrors the OBCW §5.1 semantic flags — never a provider identity.
public struct PrecipitationQuality: OptionSet, Equatable, Sendable {
    public let rawValue: UInt32
    public init(rawValue: UInt32) { self.rawValue = rawValue }

    public static let observed = PrecipitationQuality(rawValue: 1 << 0)
    public static let forecast = PrecipitationQuality(rawValue: 1 << 1)
    public static let partialCoverage = PrecipitationQuality(rawValue: 1 << 2)
    public static let degraded = PrecipitationQuality(rawValue: 1 << 3)
}

/// One frame, cropped to the corridor: a regular microdegree lattice of canonical 4-bit intensities.
///
/// The lattice is stated exactly (south/west edge plus per-axis strides plus cell counts) rather
/// than as a bbox, so the OBCW re-encode is a copy: OBCW's affine cell lookup over
/// `[south, north) x [west, east)` reproduces this lattice precisely when `north = south +
/// height * latitudeStride`. Nothing here is resampled, smoothed or interpolated.
public struct PrecipitationCrop: Equatable, Sendable {
    /// The real upstream validity time. A four-hour-old observation keeps its four-hour-old stamp.
    public var validAt: Date
    public var southMicrodegrees: Int64
    public var westMicrodegrees: Int64
    public var latitudeStrideMicrodegrees: UInt32
    public var longitudeStrideMicrodegrees: UInt32
    public var width: Int
    public var height: Int
    /// Nominal source ground resolution in metres, for truthful UI — not a projection instruction.
    public var cellSizeMetres: UInt16
    public var quality: PrecipitationQuality
    /// Row-major, rows advancing north, `width * height` canonical intensity codes.
    public var cells: [UInt8]

    public init(
        validAt: Date, southMicrodegrees: Int64, westMicrodegrees: Int64,
        latitudeStrideMicrodegrees: UInt32, longitudeStrideMicrodegrees: UInt32,
        width: Int, height: Int, cellSizeMetres: UInt16, quality: PrecipitationQuality,
        cells: [UInt8]
    ) {
        self.validAt = validAt
        self.southMicrodegrees = southMicrodegrees
        self.westMicrodegrees = westMicrodegrees
        self.latitudeStrideMicrodegrees = latitudeStrideMicrodegrees
        self.longitudeStrideMicrodegrees = longitudeStrideMicrodegrees
        self.width = width
        self.height = height
        self.cellSizeMetres = cellSizeMetres
        self.quality = quality
        self.cells = cells
    }

    public var bounds: WeatherBoundingBox {
        WeatherBoundingBox(
            southMicrodegrees: southMicrodegrees,
            westMicrodegrees: westMicrodegrees,
            northMicrodegrees: southMicrodegrees + Int64(height) * Int64(latitudeStrideMicrodegrees),
            eastMicrodegrees: westMicrodegrees + Int64(width) * Int64(longitudeStrideMicrodegrees))
    }

    /// True when any in-bounds cell is the no-data intensity — the honest source of OBCW's
    /// partial-coverage flag. No-data is never dry and never an alert-clear signal.
    public var hasNoDataCells: Bool { cells.contains(OBCPrecipitationTileCodec.noData) }
}

/// The precipitation the corridor was answered with, with everything needed to label it truthfully.
///
/// There is nothing here naming a *product*, because there is no longer a product: one dataset, one
/// lattice, one generation (#1244). What remains is provenance — which generation answered, what its
/// upstream run was, when it stops being usable, and every source that may have painted a cell.
public struct PrecipitationSelection: Equatable, Sendable {
    /// The manifest's generation string. Provenance and cache-key material only — never branched on.
    public var generation: String
    /// The stated ground resolution of the lattice, for truthful UI.
    public var nominalCellMetres: UInt16
    /// Every credit the mosaic owes. Plural because the dataset is a mosaic and no per-cell
    /// provenance exists to narrow it to one.
    public var attributions: [WeatherAttribution]
    /// Upstream run/reference time of the generation.
    public var referenceTime: Date
    /// When the baker produced this generation.
    public var generatedAt: Date
    /// The moment the generation must stop being used if no fresh manifest replaced it.
    public var stalenessDeadline: Date
    public var crops: [PrecipitationCrop]

    public init(
        generation: String, nominalCellMetres: UInt16, attributions: [WeatherAttribution],
        referenceTime: Date, generatedAt: Date, stalenessDeadline: Date,
        crops: [PrecipitationCrop]
    ) {
        self.generation = generation
        self.nominalCellMetres = nominalCellMetres
        self.attributions = attributions
        self.referenceTime = referenceTime
        self.generatedAt = generatedAt
        self.stalenessDeadline = stalenessDeadline
        self.crops = crops
    }
}

/// Why a corridor has no rain map. Every case is a *state the rider is told about* (WX11 renders
/// the explicit no-rain-map screen, WX13 the diagnostics) — never a silent empty map, and never a
/// dry claim.
///
/// The first three mirror ``WeatherPlanOutcome`` exactly, and that is the point: the manifest reader
/// already refuses to collapse "off the map", "no source here ever" and "no weather at all" into one
/// another, so the rendering-facing type must not either. **None of them is "no rain"** — a genuinely
/// dry corridor produces an all-zero frame in the bundle, not an absent one.
///
/// `Codable` because the WX13 history ring persists it: the alternative — flattening it to
/// `String(describing:)` on the way in — printed Swift's debug spelling on glass and destroyed the
/// one associated value a reader wants as a time (#1198 review).
public enum NoRainMapReason: Codable, Equatable, Sendable {
    /// The corridor is off the dataset's lattice, or is not a window this client will interpret.
    case outOfDomain
    /// On the lattice, but every row it touches is outside the manifest's `covered_rows`: no source
    /// reaches it in any frame, ever, so every cell would decode to "we do not know".
    case uncovered
    /// The generation is past its `stale_after` and no fresher manifest replaced it.
    case expired(staleAfter: Date)
    /// The manifest could not be fetched or parsed: a service outage, cleanly degraded.
    case serviceUnavailable
    /// Every frame this generation publishes is outside the window the rain map answers — too old
    /// to be a current observation, or further ahead than ``WeatherCorridor/horizon``. **Nothing
    /// failed**; the data on offer is about a different time. Distinct from ``framesUnavailable``
    /// because wearing that label would say "failed" about a service that answered perfectly.
    case outsideWindow
    /// The manifest answered, but not one frame's shards could be fetched and verified.
    case framesUnavailable
}

/// The complete weather state one job produced: an hourly section that always stands on its own and
/// a precipitation section that may honestly be absent.
///
/// The asymmetry is deliberate and is the epic's rule: a degraded or missing rain product never
/// discards a valid hourly forecast, and a missing hourly forecast is a failed job (there is no
/// bundle to send without it).
public struct WeatherState: Equatable, Sendable {
    public var hourly: HourlyForecast
    public var precipitation: PrecipitationSelection?
    public var noRainMapReason: NoRainMapReason?
    /// Every attribution that must be displayed for this state, in a stable order.
    public var attributions: [WeatherAttribution]
    public var diagnostics: WeatherDiagnostics

    public init(
        hourly: HourlyForecast, precipitation: PrecipitationSelection?,
        noRainMapReason: NoRainMapReason?, attributions: [WeatherAttribution],
        diagnostics: WeatherDiagnostics
    ) {
        self.hourly = hourly
        self.precipitation = precipitation
        self.noRainMapReason = noRainMapReason
        self.attributions = attributions
        self.diagnostics = diagnostics
    }
}

/// What the job did, for WX13's diagnostics screen and for tests to assert on. Diagnostics are
/// evidence, never control flow.
public struct WeatherDiagnostics: Equatable, Sendable {
    /// HTTP requests issued against the OBC weather service, in order.
    public var serviceRequests: Int
    /// Bytes read from the OBC weather service (Range reads included).
    public var serviceBytes: Int
    /// Manifest frames this build could not make sense of and skipped. Never fatal — one bad frame
    /// must not cost a rider the whole timeline — but never silent either.
    public var skippedManifestFrames: Int
    /// `(frame, shard)` pairs the manifest's presence bitmap said were dry, so nothing was fetched
    /// for them and intensity 0 was painted instead. Evidence that a dry map came from the bitmap
    /// rather than from a failure nobody noticed.
    public var dryShards: Int
    /// Shards that failed to fetch or verify. **One is a hole in one frame, never a failed job** —
    /// the cells it would have painted stay no-data, which is distinguishable from dry at every
    /// layer, so counting a failure here can never make an outage look rain-free.
    public var failedShards: Int
    /// Shards whose manifest entry says a radar painted them, rather than model fill. Per shard
    /// because that is where the fact is true — and it stays a **counter**: OBCW carries one quality
    /// flag per *frame*, so no per-shard bit may set it.
    public var observedShards: Int
    /// Planned frames not fetched because their validity is outside
    /// `[now - maximumObservationAge, now + horizon]`. Nothing failed; the frames are about a
    /// different time.
    public var framesOutsideWindow: Int
    /// Frames dropped because the finished bundle would otherwise exceed the producer cap.
    public var droppedOversizeFrames: Int
    /// True when the manifest's own `generated_at` is meaningfully in this device's future, which
    /// means the clock cannot be trusted for freshness arithmetic. Surfaced, never silently
    /// compensated for.
    public var clockSkewSuspected: Bool

    public init(
        serviceRequests: Int = 0, serviceBytes: Int = 0, skippedManifestFrames: Int = 0,
        dryShards: Int = 0, failedShards: Int = 0, observedShards: Int = 0,
        framesOutsideWindow: Int = 0, droppedOversizeFrames: Int = 0,
        clockSkewSuspected: Bool = false
    ) {
        self.serviceRequests = serviceRequests
        self.serviceBytes = serviceBytes
        self.skippedManifestFrames = skippedManifestFrames
        self.dryShards = dryShards
        self.failedShards = failedShards
        self.observedShards = observedShards
        self.framesOutsideWindow = framesOutsideWindow
        self.droppedOversizeFrames = droppedOversizeFrames
        self.clockSkewSuspected = clockSkewSuspected
    }
}
