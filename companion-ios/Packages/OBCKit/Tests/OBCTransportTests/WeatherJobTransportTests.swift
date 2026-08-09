import Foundation
import Testing
import OBCWeather
@testable import OBCTransport

// The WX9 transport pieces that are testable without a radio: the discovery-intent policy's
// upload lane and standing watch, the persisted flags, and the wire → domain snapshot mapping.

@Suite("BLE weather upload intent")
struct WeatherUploadPolicyTests {
    private let known = UUID()
    private let stranger = UUID()

    @Test func uploadFromIdleConnectsDirectAndOwnsTheConnection() {
        var policy = BLEDiscoveryIntentPolicy()
        let action = policy.requestWeatherUpload(knownPeripheralID: known, connectedPeripheralID: nil)
        #expect(action == .connectDirect)
        #expect(policy.phase == .connecting(peripheralID: known, owner: .weatherRequest))
        policy.didConnect(peripheralID: known)
        #expect(policy.connectionOwnership == .weatherRequest)
        // Completion disconnects the connection it created…
        let disconnects = policy.finishWeatherUpload()
        #expect(disconnects)
        policy.didDisconnect()
        #expect(policy.phase == .idle)
    }

    @Test func uploadRidesAConnectedForegroundSessionAndNeverDisconnectsIt() {
        var policy = BLEDiscoveryIntentPolicy()
        _ = policy.requestForeground()
        _ = policy.discovered(peripheralID: known, knownPeripheralID: known)
        policy.didConnect(peripheralID: known)
        let action = policy.requestWeatherUpload(knownPeripheralID: known, connectedPeripheralID: known)
        #expect(action == .uploadOnExistingConnection)
        let disconnects = policy.finishWeatherUpload()
        #expect(!disconnects, "a user-owned session is never torn down by the job")
        #expect(policy.connectionOwnership == .foreground)
    }

    @Test func uploadWaitsOutAForegroundConnectInFlight() {
        var policy = BLEDiscoveryIntentPolicy()
        _ = policy.requestForeground()
        _ = policy.discovered(peripheralID: known, knownPeripheralID: known)
        let action = policy.requestWeatherUpload(knownPeripheralID: known, connectedPeripheralID: nil)
        #expect(action == .waitForCurrentConnection)
    }

    @Test func aSharedWeatherConnectionDisconnectsOnlyWhenBothLegsAreDone() {
        var policy = BLEDiscoveryIntentPolicy()
        _ = policy.requestWeather(knownPeripheralID: known, connectedPeripheralID: nil)
        _ = policy.discovered(peripheralID: known, knownPeripheralID: known)
        policy.didConnect(peripheralID: known)
        _ = policy.requestWeatherUpload(knownPeripheralID: known, connectedPeripheralID: known)
        // The read finishing must not drop the link the upload still needs — and vice versa.
        let readDisconnects = policy.finishWeatherRequest()
        #expect(!readDisconnects)
        let uploadDisconnects = policy.finishWeatherUpload()
        #expect(uploadDisconnects)
    }

    @Test func restorationAdoptsOnlyTheKnownPeripheral() {
        var policy = BLEDiscoveryIntentPolicy()
        let ignored = policy.restoreWeatherUpload(
            restoredPeripheralIDs: [stranger], knownPeripheralID: known)
        #expect(ignored == nil)
        #expect(!policy.weatherUploadPending)
        let adopted = policy.restoreWeatherUpload(
            restoredPeripheralIDs: [stranger, known], knownPeripheralID: known)
        #expect(adopted == known)
        #expect(policy.weatherUploadPending)
        #expect(policy.phase == .connecting(peripheralID: known, owner: .weatherRequest))
    }

    @Test func radioLossClearsTheUploadButNotTheStandingWatch() {
        var policy = BLEDiscoveryIntentPolicy()
        policy.setWeatherWatch(true)
        _ = policy.requestWeatherUpload(knownPeripheralID: known, connectedPeripheralID: nil)
        policy.radioBecameUnavailable()
        #expect(!policy.weatherUploadPending)
        #expect(policy.weatherWatchArmed, "the watch is a standing preference, not an in-flight op")
        #expect(policy.phase == .idle)
        // The transport re-scans on power-on; noteScanning re-raises the phase for it.
        policy.noteScanning()
        #expect(policy.phase == .scanning)
    }
}

@Suite("BLE standing weather watch")
struct WeatherWatchPolicyTests {
    private let known = UUID()
    private let stranger = UUID()

    @Test func armingTheWatchScansWeatherOnlyWhenNothingElseWantsTheRadio() {
        var policy = BLEDiscoveryIntentPolicy()
        #expect(policy.scanServices.isEmpty)
        policy.setWeatherWatch(true)
        #expect(policy.scanServices == [.weatherRequest])
        #expect(policy.phase == .scanning)
        _ = policy.requestForeground()
        #expect(policy.scanServices == [.control, .weatherRequest])
    }

    @Test func aWatchMatchOnTheKnownPeripheralStartsAnAutonomousRead() {
        var policy = BLEDiscoveryIntentPolicy()
        policy.setWeatherWatch(true)
        let action = policy.discovered(peripheralID: known, knownPeripheralID: known)
        #expect(action == .connectForWeatherRead)
        #expect(policy.weatherRequestPending, "the watch raises the read intent itself")
        #expect(policy.phase == .connecting(peripheralID: known, owner: .weatherRequest))
    }

    @Test func aWatchMatchOnAStrangerIsIgnored() {
        var policy = BLEDiscoveryIntentPolicy()
        policy.setWeatherWatch(true)
        let action = policy.discovered(peripheralID: stranger, knownPeripheralID: known)
        #expect(action == .ignore)
        #expect(!policy.weatherRequestPending)
    }

    @Test func foregroundCancelKeepsTheWatchScanAlive() {
        var policy = BLEDiscoveryIntentPolicy()
        policy.setWeatherWatch(true)
        _ = policy.requestForeground()
        let disconnects = policy.cancelForeground()
        #expect(!disconnects)
        #expect(policy.phase == .scanning, "the watch keeps the radio after a foreground suspend")
        #expect(policy.scanServices == [.weatherRequest])
    }

    @Test func aFinishedReadFallsBackToTheWatchScanNotIdle() {
        var policy = BLEDiscoveryIntentPolicy()
        policy.setWeatherWatch(true)
        _ = policy.discovered(peripheralID: known, knownPeripheralID: known)
        policy.didConnect(peripheralID: known)
        let disconnects = policy.finishWeatherRequest()
        #expect(disconnects)
        policy.didDisconnect()
        #expect(policy.phase == .scanning)
        #expect(policy.scanServices == [.weatherRequest])
    }

    @Test func disarmingTheWatchReturnsToIdle() {
        var policy = BLEDiscoveryIntentPolicy()
        policy.setWeatherWatch(true)
        policy.setWeatherWatch(false)
        #expect(policy.phase == .idle)
        #expect(policy.scanServices.isEmpty)
    }
}

@Suite("BLE weather persistence additions")
struct WeatherUploadStoreTests {
    @Test func uploadDeadlineAndWatchFlagRoundTripAndClear() throws {
        let suite = "obc-wx9-store-tests-\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = UserDefaultsBLEDiscoveryStore(defaults: defaults)

        #expect(store.weatherUploadRestorationDeadline() == nil)
        let deadline = Date().addingTimeInterval(90)
        store.armWeatherUploadRestoration(until: deadline)
        #expect(store.weatherUploadRestorationDeadline() == deadline)
        store.clearWeatherUploadRestoration()
        #expect(store.weatherUploadRestorationDeadline() == nil)

        #expect(!store.weatherWatchArmed())
        store.setWeatherWatchArmed(true)
        #expect(store.weatherWatchArmed())
        store.setWeatherWatchArmed(false)
        #expect(!store.weatherWatchArmed())
    }
}

@Suite("Weather context → job snapshot mapping")
struct WeatherSnapshotMappingTests {
    @Test func aFullContextMapsEveryGroupThroughItsAccessor() {
        let context = WeatherRequestContext(
            validity: [.position, .bearing, .speed, .bundle, .route],
            reason: [.scheduled, .retry],
            refreshRaw: 2,
            requestID: 77,
            latitudeMicrodegrees: -49_330_889,
            longitudeMicrodegrees: -72_886_121,
            fixUTCSeconds: 1_769_999_990,
            bearingWireDegrees: 271,
            speedDeciMetersPerSecond: 64,
            routeWireID: 3,
            bundleWireGeneration: 17,
            bundleWireGeneratedAtSeconds: 1_769_000_000,
            bundleWireCRC32: 0xDEAD_BEEF)
        let readAt = Date(timeIntervalSince1970: 1_770_000_000)
        let snapshot = WeatherDeviceRequestSnapshot(context: context, readAt: readAt)

        #expect(snapshot.requestID == 77)
        #expect(snapshot.latitudeMicrodegrees == -49_330_889)
        #expect(snapshot.longitudeMicrodegrees == -72_886_121)
        #expect(snapshot.fixUnixSeconds == 1_769_999_990)
        #expect(snapshot.bearingDegrees == 271)
        #expect(snapshot.speedMetresPerSecond == 6.4)
        #expect(snapshot.routeID == 3)
        #expect(snapshot.heldBundleGeneration == 17)
        #expect(snapshot.heldBundleGeneratedAtUnixSeconds == 1_769_000_000)
        #expect(snapshot.reasonRawValue == 0b101)
        #expect(snapshot.readAt == readAt)
        // And the assembler-facing view carries the coordinate as degrees.
        let request = snapshot.weatherRequest
        #expect(request.position?.latitude == -49.330889)
        #expect(request.bearingDegrees == 271)
    }

    @Test func clearedGroupsMapToNilNeverToZeroWithMeaning() {
        // The wire fields all carry values; only the flags say they mean something. A snapshot
        // built from a flagless context must not invent a fix at the equator or generation 0.
        let context = WeatherRequestContext(
            validity: [], reason: [], requestID: 5,
            latitudeMicrodegrees: 47_000_000, longitudeMicrodegrees: 8_000_000,
            bundleWireGeneration: 9)
        let snapshot = WeatherDeviceRequestSnapshot(context: context, readAt: Date())
        #expect(snapshot.latitudeMicrodegrees == nil)
        #expect(snapshot.longitudeMicrodegrees == nil)
        #expect(snapshot.heldBundleGeneration == nil)
        #expect(snapshot.bearingDegrees == nil)
        #expect(snapshot.routeID == nil)
        #expect(snapshot.weatherRequest.position == nil)
        // No held bundle → the first upload carries generation 1.
        #expect(snapshot.nextGeneration == 1)
    }

    @Test func errorMappingSeparatesReproducibleFromRetryable() {
        // §11.5's `error` (arrived intact, not a bundle) must surface as the one reproducible
        // case; wire corruption must not — resending corrupted-in-flight bytes is the fix.
        #expect(WeatherDeviceLinkError(uploadError: .rejected) == .bundleRejected)
        #expect(WeatherDeviceLinkError(uploadError: .crcMismatch) == .connectionDropped)
        #expect(WeatherDeviceLinkError(uploadError: .timedOut) == .timedOut)
        #expect(WeatherDeviceLinkError(uploadError: .noKnownBondedPeripheral) == .noBondedDevice)
        #expect(WeatherDeviceLinkError(readError: .truncated) == .malformedContext)
        #expect(WeatherDeviceLinkError(readError: .connectionDropped) == .connectionDropped)
        #expect(WeatherDeviceLinkError(readError: .noKnownBondedPeripheral) == .noBondedDevice)
    }
}
