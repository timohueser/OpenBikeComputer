import Foundation
import OBCDomain

// The **Weather Request** contract (spec §11, WX3 / #1188) — the Swift mirror of
// `obc_ble::weather_request`, field-for-field. No CoreBluetooth here: this is pure codec plus the
// one-shot's result/error vocabulary, so it stays host-testable. `specs/vectors/weather-request-*`
// pins both halves against the same bytes.
//
// The shape of the exchange, and why it is cheap enough for a phone's background budget: the
// device swaps its *advertised* service UUID to the Weather Request service while a refresh is due;
// iOS wakes on the service match, connects, reads **one** `WeatherRequestContext` — where the rider
// is, where they are heading, what bundle they already hold — and disconnects. BLE is not held
// across the HTTP that follows. The bundle then rides back as `ObjectType.weatherBundle` on the
// ordinary reliable CoC, stamped with `requestID` so the two connections correlate.

/// Which optional groups of a `WeatherRequestContext` carry real values.
///
/// An `OptionSet` rather than named `Bool`s because the raw word must survive a round-trip
/// verbatim: **unknown bits are ignored, not rejected** — they are how a later firmware says
/// something this build was never going to act on, and refusing a whole read over one would strand
/// a rider's forecast on a bit nobody needed.
public struct WeatherRequestValidity: OptionSet, Equatable, Sendable {
    public let rawValue: UInt16
    public init(rawValue: UInt16) { self.rawValue = rawValue }

    /// The latitude / longitude / fix-time group carries a real GPS fix.
    public static let position = WeatherRequestValidity(rawValue: 1 << 0)
    /// The travel bearing is trustworthy (moving, with a course the device believes).
    public static let bearing = WeatherRequestValidity(rawValue: 1 << 1)
    /// The ground speed is trustworthy.
    public static let speed = WeatherRequestValidity(rawValue: 1 << 2)
    /// The bundle generation / timestamp / CRC group describes a bundle the device has validated
    /// and selected. Clear means *no usable bundle on the card* — not "generation 0".
    public static let bundle = WeatherRequestValidity(rawValue: 1 << 3)
    /// The route id names the active route object.
    public static let route = WeatherRequestValidity(rawValue: 1 << 4)
}

/// Why a request is due — advisory scheduling help for the phone, never an upload gate. A phone
/// that recognises none of the bits still performs the full fetch.
public struct WeatherRequestReason: OptionSet, Equatable, Sendable {
    public let rawValue: UInt16
    public init(rawValue: UInt16) { self.rawValue = rawValue }

    /// The configured refresh interval elapsed during an active ride.
    public static let scheduled = WeatherRequestReason(rawValue: 1 << 0)
    /// The rider opened Weather — treat as urgent.
    public static let urgent = WeatherRequestReason(rawValue: 1 << 1)
    /// A previous attempt failed; this is a step on the 5/10/20-minute retry ladder.
    public static let retry = WeatherRequestReason(rawValue: 1 << 2)
    /// There is no usable bundle at all, or the active one has expired.
    public static let noBundle = WeatherRequestReason(rawValue: 1 << 3)
    /// The rider has travelled outside the active bundle's covered corridor.
    public static let outOfArea = WeatherRequestReason(rawValue: 1 << 4)
    /// The held bundle contains hourly data but no rain frames; a manifest probe alone cannot prove
    /// that recreating it would be unchanged.
    public static let hourlyOnly = WeatherRequestReason(rawValue: 1 << 5)
}

/// The one value the companion reads before it disconnects — **52 little-endian bytes** describing
/// the request and the rider (spec §11).
///
/// ```text
///    0  u8   version = 1
///    1  u8   encoded_len = 52          the writer's own declared length
///    2  u16  validity flags
///    4  u16  reason flags
///    6  u8   refresh (WeatherRefresh, raw — unknown values tolerated)
///    7  u8   reserved = 0
///    8  u32  request_id
///   12  i32  lat_udeg                  ┐
///   16  i32  lon_udeg                  ├ .position
///   20  i64  fix_utc                   ┘
///   28  u16  bearing_deg 0…359         .bearing
///   30  u16  speed_deci_ms             .speed
///   32  u16  route_id                  .route
///   34  u16  reserved = 0
///   36  u32  bundle_generation         ┐
///   40  i64  bundle_generated_at       ├ .bundle
///   48  u32  bundle_crc32              ┘
/// ```
///
/// Optional groups are guarded by `validity` bits rather than by sentinel values, which is why the
/// group accessors below (`fix`, `bundle`, `bearingDegrees`, …) are the intended read path: reading
/// the flat storage directly would let "no fix" pass as the equator and "no bundle" as generation
/// 0. Widths mirror the OBCW header exactly (`i32` microdegrees, `i64` UTC seconds, `u32`
/// generation/CRC) so a value round-trips into a bundle header without narrowing.
public struct WeatherRequestContext: Equatable, Sendable {
    /// The layout version this build writes. v1 is exactly `encodedLength` bytes.
    public static let currentVersion: UInt8 = 1
    /// v1's exact encoded length, and the value byte 1 carries.
    public static let encodedLength = 52
    /// The shortest read that can be classified at all: the version/length prefix itself.
    public static let minimumEncodedLength = 2

    /// Layout version, reported as the device stated it — never normalised away, so a diagnostic
    /// can say which generation answered.
    public var version: UInt8
    public var validity: WeatherRequestValidity
    public var reason: WeatherRequestReason
    /// The device's configured refresh interval **as the byte arrived**, echoed here so the phone
    /// can schedule its own work without also reading Config.
    ///
    /// Raw for the same reason `validity` and `reason` are raw words: this is a device → phone
    /// read, so a value this build does not know is a *newer firmware*, not a malformed one. Use
    /// `refresh` for the typed view and keep this for a verbatim round-trip (spec §11.8).
    public var refreshRaw: UInt8
    /// The request nonce, stamped into the OBCW header's `request_id`. Monotonic per device boot and
    /// **stable across the retry ladder**, so retries of one request stay one request.
    public var requestID: UInt32

    // The flat wire storage. Each field is always on the wire; only its flag says whether it means
    // anything. Read them through the group accessors below.
    public var latitudeMicrodegrees: Int32
    public var longitudeMicrodegrees: Int32
    public var fixUTCSeconds: Int64
    public var bearingWireDegrees: UInt16
    public var speedDeciMetersPerSecond: UInt16
    public var routeWireID: UInt16
    public var bundleWireGeneration: UInt32
    public var bundleWireGeneratedAtSeconds: Int64
    public var bundleWireCRC32: UInt32

    public init(
        version: UInt8 = WeatherRequestContext.currentVersion,
        validity: WeatherRequestValidity = [],
        reason: WeatherRequestReason = [],
        refreshRaw: UInt8 = WeatherRefresh.deviceDefault.rawValue,
        requestID: UInt32 = 0,
        latitudeMicrodegrees: Int32 = 0,
        longitudeMicrodegrees: Int32 = 0,
        fixUTCSeconds: Int64 = 0,
        bearingWireDegrees: UInt16 = 0,
        speedDeciMetersPerSecond: UInt16 = 0,
        routeWireID: UInt16 = 0,
        bundleWireGeneration: UInt32 = 0,
        bundleWireGeneratedAtSeconds: Int64 = 0,
        bundleWireCRC32: UInt32 = 0
    ) {
        self.version = version
        self.validity = validity
        self.reason = reason
        self.refreshRaw = refreshRaw
        self.requestID = requestID
        self.latitudeMicrodegrees = latitudeMicrodegrees
        self.longitudeMicrodegrees = longitudeMicrodegrees
        self.fixUTCSeconds = fixUTCSeconds
        self.bearingWireDegrees = bearingWireDegrees
        self.speedDeciMetersPerSecond = speedDeciMetersPerSecond
        self.routeWireID = routeWireID
        self.bundleWireGeneration = bundleWireGeneration
        self.bundleWireGeneratedAtSeconds = bundleWireGeneratedAtSeconds
        self.bundleWireCRC32 = bundleWireCRC32
    }

    /// The resting value: a well-formed v1 context with nothing valid and no reason. This is what
    /// the characteristic holds before any request is raised, so a peer that reads it out of turn
    /// gets a structurally valid "nothing is due" rather than stale rider coordinates.
    public static let empty = WeatherRequestContext()

    // MARK: The optional groups

    /// The device's configured refresh interval, or `nil` when it named one this build does not
    /// know — the *read* direction of §11.8.
    ///
    /// `nil` here means **unknown**, not `.off` and not the default: a phone that collapsed it to
    /// either would misreport the rider's own setting back to them. It is not an error, though —
    /// tomorrow's firmware appending a fifth interval must not cost a shipped app its entire
    /// weather path over one byte it was never going to act on.
    public var refresh: WeatherRefresh? { WeatherRefresh(wireByte: refreshRaw) }

    /// Where the rider was, or `nil` when `validity` claims no fix. Absence is a cleared flag, never
    /// a sentinel — without this guard "no fix yet" reads as the Gulf of Guinea.
    public var fix: Fix? {
        guard validity.contains(.position) else { return nil }
        return Fix(
            latitudeMicrodegrees: latitudeMicrodegrees,
            longitudeMicrodegrees: longitudeMicrodegrees,
            utc: Date(timeIntervalSince1970: TimeInterval(fixUTCSeconds))
        )
    }

    /// Travel bearing in whole degrees `0…359`, or `nil` when the device does not vouch for it.
    public var bearingDegrees: UInt16? { validity.contains(.bearing) ? bearingWireDegrees : nil }

    /// Ground speed in metres per second, or `nil` when the device does not vouch for it. The wire
    /// carries tenths.
    public var speedMetersPerSecond: Double? {
        validity.contains(.speed) ? Double(speedDeciMetersPerSecond) / 10 : nil
    }

    /// The active route's object id, or `nil` when no route is being navigated. `nil` rather than
    /// `0`, which is a legal object id.
    public var routeID: UInt16? { validity.contains(.route) ? routeWireID : nil }

    /// The bundle the device already holds, or `nil` when it holds none. `nil` rather than
    /// generation 0 — the phone must be able to tell "has nothing" from "has the first one".
    public var bundle: HeldBundle? {
        guard validity.contains(.bundle) else { return nil }
        return HeldBundle(
            generation: bundleWireGeneration,
            generatedAt: Date(timeIntervalSince1970: TimeInterval(bundleWireGeneratedAtSeconds)),
            crc32: bundleWireCRC32
        )
    }

    /// A GPS fix as the request context carries it.
    public struct Fix: Equatable, Sendable {
        public var latitudeMicrodegrees: Int32
        public var longitudeMicrodegrees: Int32
        public var utc: Date

        public init(latitudeMicrodegrees: Int32, longitudeMicrodegrees: Int32, utc: Date) {
            self.latitudeMicrodegrees = latitudeMicrodegrees
            self.longitudeMicrodegrees = longitudeMicrodegrees
            self.utc = utc
        }

        public var latitude: Double { Double(latitudeMicrodegrees) / 1_000_000 }
        public var longitude: Double { Double(longitudeMicrodegrees) / 1_000_000 }
    }

    /// The weather bundle the device has validated and selected.
    public struct HeldBundle: Equatable, Sendable {
        public var generation: UInt32
        public var generatedAt: Date
        public var crc32: UInt32

        public init(generation: UInt32, generatedAt: Date, crc32: UInt32) {
            self.generation = generation
            self.generatedAt = generatedAt
            self.crc32 = crc32
        }
    }

    // MARK: Codec

    // The one place the offsets are written down, so `encode` and `decode` cannot drift apart in a
    // way only a byte-level test would catch.
    private enum Offset {
        static let version = 0
        static let encodedLength = 1
        static let validity = 2
        static let reason = 4
        static let refresh = 6
        static let reserved0 = 7
        static let requestID = 8
        static let latitude = 12
        static let longitude = 16
        static let fixUTC = 20
        static let bearing = 28
        static let speed = 30
        static let routeID = 32
        static let reserved1 = 34
        static let bundleGeneration = 36
        static let bundleGeneratedAt = 40
        static let bundleCRC32 = 48
    }

    /// Encode the fixed 52-byte v1 value. The device is the only writer on the real link; this
    /// exists so the round-trip and shared-vector tests can pin the layout from this side too.
    public func encode() -> Data {
        var bytes = [UInt8](repeating: 0, count: Self.encodedLength)
        bytes[Offset.version] = version
        bytes[Offset.encodedLength] = UInt8(Self.encodedLength)
        bytes.writeLE(validity.rawValue, at: Offset.validity)
        bytes.writeLE(reason.rawValue, at: Offset.reason)
        bytes[Offset.refresh] = refreshRaw
        bytes[Offset.reserved0] = 0
        bytes.writeLE(requestID, at: Offset.requestID)
        bytes.writeLE(UInt32(bitPattern: latitudeMicrodegrees), at: Offset.latitude)
        bytes.writeLE(UInt32(bitPattern: longitudeMicrodegrees), at: Offset.longitude)
        bytes.writeLE(UInt64(bitPattern: fixUTCSeconds), at: Offset.fixUTC)
        bytes.writeLE(bearingWireDegrees, at: Offset.bearing)
        bytes.writeLE(speedDeciMetersPerSecond, at: Offset.speed)
        bytes.writeLE(routeWireID, at: Offset.routeID)
        bytes.writeLE(UInt16(0), at: Offset.reserved1)
        bytes.writeLE(bundleWireGeneration, at: Offset.bundleGeneration)
        bytes.writeLE(UInt64(bitPattern: bundleWireGeneratedAtSeconds), at: Offset.bundleGeneratedAt)
        bytes.writeLE(bundleWireCRC32, at: Offset.bundleCRC32)
        return Data(bytes)
    }

    /// Decode a context read.
    ///
    /// The read is **length-declared**: byte 1 states how many bytes the writer produced, and a read
    /// that delivered fewer is refused rather than half-decoded. Bytes past this version's 52 are
    /// **ignored**, so a future firmware that appends a field keeps working against a shipped app —
    /// the same append-only rule the identity read and Config live under.
    ///
    /// Reserved bytes, unknown validity/reason bits and an unknown `refresh` byte are all
    /// **ignored, not rejected**. The refresh byte belongs in that list precisely because this is a
    /// device → phone read (§11.8): a value this build cannot name is a newer firmware naming a
    /// newer interval, and failing the read over it would let one ordinary enum append switch
    /// weather off permanently on every already-shipped app. It rides through verbatim and reads as
    /// `nil` from `refresh`.
    public init(decoding data: Data) throws {
        guard data.count >= Self.minimumEncodedLength else { throw WeatherRequestError.truncated }
        let base = data.startIndex
        let declared = Int(data[base + Offset.encodedLength])
        // A writer claiming fewer bytes than v1 defines is not an *old* writer — v1 is the first
        // version — so it is malformed rather than something to decode leniently.
        guard declared >= Self.encodedLength, data.count >= declared else {
            throw WeatherRequestError.invalidLength
        }
        self.init(
            version: data[base + Offset.version],
            validity: WeatherRequestValidity(rawValue: data.readLE(at: base + Offset.validity)),
            reason: WeatherRequestReason(rawValue: data.readLE(at: base + Offset.reason)),
            refreshRaw: data[base + Offset.refresh],
            requestID: data.readLE(at: base + Offset.requestID),
            latitudeMicrodegrees: Int32(bitPattern: data.readLE(at: base + Offset.latitude)),
            longitudeMicrodegrees: Int32(bitPattern: data.readLE(at: base + Offset.longitude)),
            fixUTCSeconds: Int64(bitPattern: data.readLE(at: base + Offset.fixUTC)),
            bearingWireDegrees: data.readLE(at: base + Offset.bearing),
            speedDeciMetersPerSecond: data.readLE(at: base + Offset.speed),
            routeWireID: data.readLE(at: base + Offset.routeID),
            bundleWireGeneration: data.readLE(at: base + Offset.bundleGeneration),
            bundleWireGeneratedAtSeconds: Int64(bitPattern: data.readLE(at: base + Offset.bundleGeneratedAt)),
            bundleWireCRC32: data.readLE(at: base + Offset.bundleCRC32)
        )
    }
}

/// Why one weather-request read did not produce a context — plus the one Config-write refusal
/// §11.8 defines (`unknownRefresh`), which shares this vocabulary because it is the same wire byte.
///
/// The decode cases split what `obc_ble`'s `DescriptorError` folds into one `Truncated`:
/// `.truncated` is "not even the length prefix arrived", `.invalidLength` is "byte 1 and the read
/// disagree". Both are refusals — nothing is half-decoded either way.
public enum WeatherRequestError: Error, Equatable, Sendable {
    /// No peripheral has ever completed an authenticated session, so there is nothing to scan for.
    case noKnownBondedPeripheral
    case bluetoothUnavailable
    case timedOut
    case connectionDropped
    /// Byte 1 declared a length below v1's 52, or more bytes than the read delivered.
    case invalidLength
    /// Fewer than the two prefix bytes arrived.
    case truncated
    /// A refresh byte this build does not know — an interval it cannot honour, not a default.
    ///
    /// The mirror of `obc_ble::descriptor::DescriptorError::UnknownRefresh`, and its own case for
    /// the same reason: it is the one decode failure whose correct handling depends on the
    /// **direction of travel** (§11.8). It is thrown by exactly one caller,
    /// `DeviceConfig.weatherRefreshToApply()` — the phone → device write. Both read directions
    /// (a context read, a Config read) report unknown and carry on, so this case never surfaces
    /// from `WeatherRequestContext(decoding:)`; a build that threw it there would take the strict
    /// rule to a direction that cannot survive it.
    case unknownRefresh(UInt8)
    case readFailed
    case cancelled
}

/// One completed weather-request read, with timing evidence from the bounded
/// discover → connect → read → disconnect attempt.
///
/// The durations use a monotonic clock and are **diagnostics only** (they feed the background
/// discovery spike's logs); the context is the entire wire value.
public struct WeatherRequestRead: Equatable, Sendable {
    public var context: WeatherRequestContext
    public var discoveryLatency: Duration
    public var connectedDuration: Duration
    /// True when the read rode an existing foreground link instead of its own connection — which is
    /// also why that link must not be dropped when the read completes.
    public var reusedForegroundConnection: Bool

    public init(
        context: WeatherRequestContext,
        discoveryLatency: Duration,
        connectedDuration: Duration,
        reusedForegroundConnection: Bool
    ) {
        self.context = context
        self.discoveryLatency = discoveryLatency
        self.connectedDuration = connectedDuration
        self.reusedForegroundConnection = reusedForegroundConnection
    }
}

public enum WeatherRequestEvent: Equatable, Sendable {
    case completed(WeatherRequestRead)
    case failed(WeatherRequestError)
}

// MARK: - Little-endian access

// Local to this independently pinned 52-byte request layout: unlike the common `Data` helpers,
// this writer targets an `[UInt8]` at fixed offsets and also owns the layout's `UInt64` fields.
extension Array where Element == UInt8 {
    fileprivate mutating func writeLE(_ value: UInt16, at index: Int) {
        self[index] = UInt8(value & 0xFF)
        self[index + 1] = UInt8((value >> 8) & 0xFF)
    }

    fileprivate mutating func writeLE(_ value: UInt32, at index: Int) {
        for byte in 0..<4 { self[index + byte] = UInt8((value >> (8 * UInt32(byte))) & 0xFF) }
    }

    fileprivate mutating func writeLE(_ value: UInt64, at index: Int) {
        for byte in 0..<8 { self[index + byte] = UInt8((value >> (8 * UInt64(byte))) & 0xFF) }
    }
}

extension Data {
    fileprivate func readLE(at index: Int) -> UInt16 {
        UInt16(self[index]) | (UInt16(self[index + 1]) << 8)
    }

    fileprivate func readLE(at index: Int) -> UInt32 {
        (0..<4).reduce(into: UInt32(0)) { $0 |= UInt32(self[index + $1]) << (8 * UInt32($1)) }
    }

    fileprivate func readLE(at index: Int) -> UInt64 {
        (0..<8).reduce(into: UInt64(0)) { $0 |= UInt64(self[index + $1]) << (8 * UInt64($1)) }
    }
}
