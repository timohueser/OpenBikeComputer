import Foundation
import OBCWeather

// The WX9 seam between the weather domain and the BLE transport. OBCWeather owns the job engine
// and must never see CoreBluetooth (its Package.swift dependency set enforces that); this file is
// where its `WeatherDeviceLink` protocol meets `BLETransport`. It lives *outside* `BLE/`
// deliberately — nothing here imports CoreBluetooth, only the transport's own public surface —
// so the CoreBluetooth seam test keeps meaning what it says.

// MARK: - The wire → domain mapping (WX9 owns this, per OBCWeather.WeatherRequest's doc)

extension WeatherDeviceRequestSnapshot {
    /// Map one decoded `weatherRequestContext` into the job's durable snapshot. The wire's
    /// validity flags become optionals here — a cleared position bit is `nil`, never the equator —
    /// and the bundle group survives as the generation/timestamp pair the §11.6 arithmetic needs.
    public init(context: WeatherRequestContext, readAt: Date) {
        let fix = context.fix
        let bundle = context.bundle
        self.init(
            requestID: context.requestID,
            latitudeMicrodegrees: fix?.latitudeMicrodegrees,
            longitudeMicrodegrees: fix?.longitudeMicrodegrees,
            fixUnixSeconds: fix.map { Int64($0.utc.timeIntervalSince1970.rounded()) },
            bearingDegrees: context.bearingDegrees.map(Double.init),
            speedMetresPerSecond: context.speedMetersPerSecond,
            routeID: context.routeID,
            heldBundleGeneration: bundle?.generation,
            heldBundleGeneratedAtUnixSeconds: bundle.map {
                Int64($0.generatedAt.timeIntervalSince1970.rounded())
            },
            reasonRawValue: context.reason.rawValue,
            readAt: readAt
        )
    }
}

extension WeatherDeviceLinkError {
    /// The read leg's error vocabulary, folded to the engine's retryable/reproducible split.
    public init(readError: WeatherRequestError) {
        switch readError {
        case .noKnownBondedPeripheral: self = .noBondedDevice
        case .bluetoothUnavailable: self = .bluetoothUnavailable
        case .timedOut: self = .timedOut
        case .connectionDropped: self = .connectionDropped
        case .invalidLength, .truncated, .unknownRefresh: self = .malformedContext
        case .readFailed: self = .connectionDropped
        case .cancelled: self = .cancelled
        }
    }

    /// The upload leg's — same split. `rejected` is the one reproducible case (§11.5 `error`);
    /// `crcMismatch` is its explicit opposite (the wire corrupted correct bytes — resend them),
    /// and `storageFull` / `notFound` are the device's situation rather than a verdict on the
    /// bundle, so they keep the bytes and come back later.
    public init(uploadError: WeatherUploadError) {
        switch uploadError {
        case .emptyPayload, .rejected: self = .bundleRejected
        case .noKnownBondedPeripheral: self = .noBondedDevice
        case .bluetoothUnavailable: self = .bluetoothUnavailable
        case .busy: self = .linkBusy
        case .timedOut: self = .timedOut
        case .connectionDropped: self = .connectionDropped
        case .crcMismatch: self = .transferCorrupted
        case .deviceBusy, .storageFull, .notFound: self = .deviceBusy
        case .cancelled: self = .cancelled
        }
    }
}

#if canImport(CoreBluetooth)
/// `BLETransport`'s conformance to the job engine's transport seam — a thin translation, all the
/// behaviour lives in the transport's bounded one-shots.
public struct WeatherBLEDeviceLink: WeatherDeviceLink {
    private let transport: BLETransport
    private let now: @Sendable () -> Date

    public init(transport: BLETransport, now: @escaping @Sendable () -> Date = Date.init) {
        self.transport = transport
        self.now = now
    }

    public func readRequestContext() async throws -> WeatherContextReadReceipt {
        do {
            let read = try await transport.readWeatherRequestContext()
            return WeatherContextReadReceipt(
                snapshot: WeatherDeviceRequestSnapshot(context: read.context, readAt: now()),
                connectedDuration: read.connectedDuration,
                reusedForegroundConnection: read.reusedForegroundConnection
            )
        } catch let error as WeatherRequestError {
            throw WeatherDeviceLinkError(readError: error)
        }
    }

    public func uploadBundle(_ bytes: Data) async throws -> WeatherBundleUploadReceipt {
        do {
            let upload = try await transport.uploadWeatherBundle(bytes)
            return WeatherBundleUploadReceipt(
                connectLatency: upload.connectLatency,
                connectedDuration: upload.connectedDuration,
                reusedForegroundConnection: upload.reusedForegroundConnection
            )
        } catch let error as WeatherUploadError {
            throw WeatherDeviceLinkError(uploadError: error)
        }
    }

    public func acknowledgeUnchanged(
        requestID: UInt32, retryAfterSeconds: UInt16
    ) async throws -> WeatherBundleUploadReceipt {
        do {
            let upload = try await transport.acknowledgeWeatherUnchanged(
                requestID: requestID, retryAfterSeconds: retryAfterSeconds)
            return WeatherBundleUploadReceipt(
                connectLatency: upload.connectLatency,
                connectedDuration: upload.connectedDuration,
                reusedForegroundConnection: upload.reusedForegroundConnection)
        } catch let error as WeatherUploadError {
            throw WeatherDeviceLinkError(uploadError: error)
        }
    }
}

/// Feeds the transport's autonomously completed context reads (background wakes, state
/// restoration — reads with no caller) into the job engine, and resumes any checkpointed job at
/// start. The composition root starts this once per launch and keeps the task alive.
public enum WeatherJobBLEBridge {
    public static func start(transport: BLETransport, engine: WeatherJobEngine) -> Task<Void, Never> {
        Task {
            // First: whatever the checkpoint says is owed (a relaunch mid-job resumes here).
            await engine.kick(.resume)
            // Then: every completed read — including the replayed latest, which is exactly how a
            // read that finished while nobody was running reaches the engine.
            for await event in transport.weatherRequestEvents {
                guard case .completed(let read) = event else { continue }
                let snapshot = WeatherDeviceRequestSnapshot(context: read.context, readAt: Date())
                // §11.4's idle attribute (validity 0, reason 0, no nonce) is what a device with
                // nothing pending answers — including the replayed *latest* event this stream
                // hands every new subscriber. Feeding it to the engine would spend a job, and a
                // diagnostics row, on a request nobody made.
                guard snapshot.carriesRequest else { continue }
                await engine.kick(.contextRead(
                    snapshot,
                    readConnectedMilliseconds: Int(read.connectedDuration / .milliseconds(1))
                ))
            }
        }
    }
}
#endif
