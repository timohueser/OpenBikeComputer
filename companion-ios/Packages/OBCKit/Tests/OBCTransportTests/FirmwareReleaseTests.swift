import Foundation
import Testing
import OBCTransport

/// The update check (#773 U4): the manifest parser, the version dialect, the cache, and the
/// pre-release channel.
///
/// **This suite is a port of the builder's `release.test.ts` matrix**, deliberately case-for-case.
/// Two parsers read the same published manifest — the Svelte builder's device step over USB and
/// this app over BLE — and the only thing keeping them from drifting is that both are pinned to
/// the same table. A case added there belongs here, and vice versa.
///
/// The behaviour with a locked decision behind it is the *refusal*: #773 states that a device
/// reporting a git hash rather than a release version is never offered an auto-update, so the tests
/// that matter most are the ones proving an unparseable version produces `unknown` and not "you're
/// out of date".
struct FirmwareReleaseTests {
    // MARK: Fixtures

    private static let manifestBody = Data(
        """
        {
          "version": "1.4.0",
          "size": 812345,
          "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          "url": "https://updates.openbikecomputer.com/fw/v1.4.0/UPDATE.BIN",
          "notes": "https://github.com/timohueser/OpenBikeComputer/releases/tag/v1.4.0",
          "signature": "ignored-by-this-parser"
        }
        """.utf8
    )

    private static func manifest(version: String, url: String = "https://x.example/UPDATE.BIN") -> Data {
        Data(
            """
            {"version":"\(version)","bytes":100,
             "sha256":"\(String(repeating: "b", count: 64))","url":"\(url)"}
            """.utf8
        )
    }

    // MARK: parseFirmwareManifest

    @Test func readsTheFieldsTheCheckNeedsAndIgnoresTheRest() throws {
        let release = try parseFirmwareManifest(Self.manifestBody)
        #expect(release.version == "1.4.0")
        #expect(release.bytes == 812_345)
        #expect(release.sha256 == String(repeating: "a", count: 64))
        #expect(release.url.absoluteString.hasSuffix("/UPDATE.BIN"))
        #expect(release.notes?.contains("releases/tag/v1.4.0") == true)
        #expect(release.notesURL != nil)
    }

    /// `bytes` is the field U3 emits; `size` is accepted as its synonym (the builder's parser
    /// takes either, and a manifest that satisfies one parser must satisfy both).
    @Test func acceptsEitherSpellingOfTheSize() throws {
        let bytes = try parseFirmwareManifest(
            Data(#"{"version":"1.0.0","bytes":42,"sha256":"\#(String(repeating: "c", count: 64))","url":"https://x/"}"#.utf8)
        )
        #expect(bytes.bytes == 42)
        let size = try parseFirmwareManifest(
            Data(#"{"version":"1.0.0","size":42,"sha256":"\#(String(repeating: "c", count: 64))","url":"https://x/"}"#.utf8)
        )
        #expect(size.bytes == 42)
    }

    /// The builder's `bad` table, verbatim in intent: nothing here is guessed at.
    @Test(arguments: [
        "not json",
        "[]",
        #"{"version":"1.4.0","size":1,"sha256":"aaaa"}"#,                                       // no url
        #"{"version":"abc1234","size":1,"sha256":"§64§","url":"https://x/"}"#,                  // not a version
        #"{"version":"1.4.0","size":0,"sha256":"§64§","url":"https://x/"}"#,                    // size 0
        #"{"version":"1.4.0","size":1,"sha256":"nope","url":"https://x/"}"#,                    // short digest
        #"{"version":"1.4.0","size":1,"sha256":"§64§","url":"http://x/"}"#,                     // not https
        #"{"version":"1.4.0","size":1.5,"sha256":"§64§","url":"https://x/"}"#,                  // not an integer
        #"{"version":"1.4.0","size":9007199254740992,"sha256":"§64§","url":"https://x/"}"#,     // not safely representable in both clients
        #"{"version":"1.4.0","size":true,"sha256":"§64§","url":"https://x/"}"#,                 // not a number
        #"{"version":"","size":1,"sha256":"§64§","url":"https://x/"}"#,                         // empty version
        #"{"size":1,"sha256":"§64§","url":"https://x/"}"#,                                      // no version
        #"{"version":"1.4.0","sha256":"§64§","url":"https://x/"}"#,                             // no size
        "42",
    ])
    func rejectsAMalformedManifestRatherThanGuessingAtIt(body: String) {
        let filled = body.replacingOccurrences(of: "§64§", with: String(repeating: "a", count: 64))
        #expect(throws: FirmwareManifestError.self) {
            _ = try parseFirmwareManifest(Data(filled.utf8))
        }
    }

    /// An uppercase digest is a spelling, not a different digest — normalised, like the builder's
    /// `.toLowerCase()`.
    @Test func normalisesTheDigestCase() throws {
        let release = try parseFirmwareManifest(
            Data(#"{"version":"1.0.0","bytes":1,"sha256":"\#(String(repeating: "A", count: 64))","url":"https://x/"}"#.utf8)
        )
        #expect(release.sha256 == String(repeating: "a", count: 64))
    }

    // MARK: The version dialect (the builder's table)

    @Test func ignoresBuildMetadataWhichIsWhatDISReportsToday() {
        #expect(FirmwareVersion.parse("1.2.0+abc1234") == FirmwareVersion(major: 1, minor: 2, patch: 0))
        #expect(FirmwareVersion.compare("1.2.0+abc1234", "1.2.0") == 0)
        #expect(FirmwareVersion.compare("v1.2.0", "1.2.0") == 0)
    }

    @Test func refusesToCompareAGitHash() {
        #expect(FirmwareVersion.parse("abc1234") == nil)
        #expect(FirmwareVersion.compare("abc1234", "1.4.0") == nil)
    }

    @Test func ordersReleasesAndTheirPreReleases() throws {
        #expect(try #require(FirmwareVersion.compare("1.3.0", "1.4.0")) < 0)
        #expect(try #require(FirmwareVersion.compare("1.4.1", "1.4.0")) > 0)
        #expect(try #require(FirmwareVersion.compare("2.0.0", "1.99.99")) > 0)
        #expect(try #require(FirmwareVersion.compare("1.4.0-rc1", "1.4.0")) < 0)
        #expect(try #require(FirmwareVersion.compare("1.4.0-rc1", "1.4.0-rc2")) < 0)
        #expect(try #require(FirmwareVersion.compare("1.4.0-rc.2", "1.4.0-rc.10")) < 0)
        #expect(try #require(FirmwareVersion.compare("1.4.0-10", "1.4.0-rc")) < 0)
    }

    /// A manifest controls one side of this comparison. Keep the full shared safe-integer range
    /// comparable without arithmetic overflow, even at its boundary.
    @Test func comparesTheLargestParseableNumericComponentWithoutTrapping() throws {
        let largest = "9007199254740991.0.0"
        #expect(try #require(FirmwareVersion.compare(largest, "0.0.0")) > 0)
        #expect(try #require(FirmwareVersion.compare("0.0.0", largest)) < 0)
    }

    /// The shapes that are *not* release versions. Each one would, if it parsed, put the app in the
    /// business of guessing what a device is running.
    @Test(arguments: [
        "", " ", "v", "1", "1.2", "1.2.3.4", "1.2.x", "1.2.-1", "v1.2.0-", "1.2.0+",
        "abc1234", "g1a2b3c4", "1.2.0-rc 1", "one.two.three", "1.2.0+build+more", "-1.2.0",
        "9007199254740992.0.0",
    ])
    func refusesEverythingThatIsNotAReleaseVersion(text: String) {
        #expect(FirmwareVersion.parse(text) == nil, "\"\(text)\" must not parse as a release version")
    }

    /// The shapes that *are*, including the ones DIS and the OBCU header actually produce.
    @Test(arguments: [
        "1.2.0", "v1.2.0", "0.0.0", "10.20.30", "1.2.0+abc1234", "v1.2.0+abc1234",
        "1.2.0-rc1", "1.2.0-rc.1+abc1234", "  1.2.0  ",
    ])
    func acceptsTheReleaseVersionsTheDeviceCanReport(text: String) {
        #expect(FirmwareVersion.parse(text) != nil, "\"\(text)\" must parse as a release version")
    }

    // MARK: updateStatus

    @Test func neverOffersAnUpdateToADeviceRunningAnUnparseableVersion() {
        // #773's locked behaviour: a probe-flashed dev build reports a hash, and the answer is
        // "cannot say" — collapsing that into "older" would push firmware onto a dev device.
        #expect(FirmwareVersion.updateStatus(running: "abc1234", latest: "1.4.0") == .unknown)
        #expect(FirmwareVersion.updateStatus(running: nil, latest: "1.4.0") == .unknown)
    }

    @Test func saysNothingAtAllWhenThereIsNoPublishedRelease() {
        #expect(FirmwareVersion.updateStatus(running: "1.3.0", latest: nil) == .noRelease)
    }

    /// The ordering the builder settled on in #1004, mirrored here: what makes a dev build
    /// undecidable is the hash it reports, not whether anything is published. The other way round
    /// would hide the "development build" state for as long as U3 hasn't published — which is
    /// exactly today.
    @Test func stillCallsADevBuildADevBuildWhenNothingIsPublished() {
        #expect(FirmwareVersion.updateStatus(running: "abc1234", latest: nil) == .unknown)
        // …but a device that has said nothing yet is not a dev build; there is simply no check.
        #expect(FirmwareVersion.updateStatus(running: nil, latest: nil) == .noRelease)
        #expect(FirmwareVersion.updateStatus(running: "", latest: nil) == .noRelease)
    }

    @Test func distinguishesOlderCurrentAndAhead() {
        #expect(FirmwareVersion.updateStatus(running: "1.3.0", latest: "1.4.0") == .available)
        #expect(FirmwareVersion.updateStatus(running: "1.4.0+deadbee", latest: "1.4.0") == .current)
        #expect(FirmwareVersion.updateStatus(running: "1.5.0", latest: "1.4.0") == .ahead)
    }

    // MARK: The fetch

    @Test func treatsA404AsNothingPublishedYetNotAnError() async throws {
        let fetcher = StubFetcher()
        fetcher.stub(UpdateChecker.manifestURL, status: 404)
        let store = InMemoryUpdateCheckStore()
        let record = try await UpdateChecker(fetcher: fetcher, store: store).check()
        #expect(record.release == nil)
        // …and it is *recorded* as an answer, so the screen doesn't re-ask on every appear until
        // U3 finally publishes something.
        #expect(store.loadCheck()?.release == nil)
        #expect(store.loadCheck() != nil)
    }

    @Test func surfacesAServerError() async {
        let fetcher = StubFetcher()
        fetcher.stub(UpdateChecker.manifestURL, status: 500)
        await #expect(throws: FirmwareManifestError.httpStatus(500)) {
            _ = try await UpdateChecker(fetcher: fetcher, store: InMemoryUpdateCheckStore()).check()
        }
    }

    @Test func parsesAPublishedManifest() async throws {
        let fetcher = StubFetcher()
        fetcher.stub(UpdateChecker.manifestURL, body: Self.manifestBody)
        let record = try await UpdateChecker(fetcher: fetcher, store: InMemoryUpdateCheckStore()).check()
        #expect(record.release?.version == "1.4.0")
    }

    /// The privacy posture is a requirement (#773): the check asks for one public file and sends
    /// nothing about the device — no query string, no second request, no pre-release probe unless
    /// the dev switch is on.
    @Test func asksForExactlyOnePublicFileAndNothingElse() async throws {
        let fetcher = StubFetcher()
        fetcher.stub(UpdateChecker.manifestURL, body: Self.manifestBody)
        _ = try await UpdateChecker(fetcher: fetcher, store: InMemoryUpdateCheckStore()).check()
        #expect(fetcher.requested == [UpdateChecker.manifestURL])
        #expect(UpdateChecker.manifestURL.query == nil)
    }

    // MARK: The cache

    @Test func cachesTheAnswerWithItsTimestamp() async throws {
        let fetcher = StubFetcher()
        fetcher.stub(UpdateChecker.manifestURL, body: Self.manifestBody)
        let store = InMemoryUpdateCheckStore()
        let checker = UpdateChecker(fetcher: fetcher, store: store)
        let taken = Date(timeIntervalSince1970: 1_700_000_000)

        #expect(checker.cachedCheck() == nil)
        _ = try await checker.check(now: taken)

        let cached = try #require(checker.cachedCheck())
        #expect(cached.release?.version == "1.4.0")
        #expect(cached.checkedAt == taken)
        #expect(cached == store.loadCheck())
    }

    @Test func agesTheCachedAnswerOut() {
        let checker = UpdateChecker(fetcher: StubFetcher(), store: InMemoryUpdateCheckStore())
        let taken = Date(timeIntervalSince1970: 1_700_000_000)
        let record = UpdateCheckRecord(release: nil, checkedAt: taken)

        #expect(checker.isFresh(record, now: taken))
        #expect(checker.isFresh(record, now: taken.addingTimeInterval(UpdateChecker.freshness - 1)))
        #expect(!checker.isFresh(record, now: taken.addingTimeInterval(UpdateChecker.freshness)))
        // A clock that moved backwards reads as stale, never as fresh forever.
        #expect(!checker.isFresh(record, now: taken.addingTimeInterval(-60)))
    }

    /// The `UserDefaults` store is the shipping one: a record written by one launch must be
    /// readable by the next, and an unreadable one must degrade to "no cache", never to a crash.
    @Test func theUserDefaultsStoreRoundTripsAndToleratesGarbage() throws {
        let suite = "obc.tests.\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = UserDefaultsUpdateCheckStore(defaults: defaults)

        #expect(store.loadCheck() == nil)
        #expect(!store.loadIncludePrereleases())

        let record = UpdateCheckRecord(
            release: try parseFirmwareManifest(Self.manifestBody),
            checkedAt: Date(timeIntervalSince1970: 1_700_000_000)
        )
        store.saveCheck(record)
        store.saveIncludePrereleases(true)
        #expect(UserDefaultsUpdateCheckStore(defaults: defaults).loadCheck() == record)
        #expect(UserDefaultsUpdateCheckStore(defaults: defaults).loadIncludePrereleases())

        defaults.set(Data([0x00, 0x01]), forKey: "obc.firmwareUpdateCheck")
        #expect(store.loadCheck() == nil, "an unreadable record is no cache, not a crash")
    }

    // MARK: The pre-release channel (the dev switch)

    @Test func ignoresThePreReleaseChannelUnlessTheSwitchIsOn() async throws {
        let fetcher = StubFetcher()
        fetcher.stub(UpdateChecker.manifestURL, body: Self.manifest(version: "1.4.0"))
        fetcher.stub(UpdateChecker.prereleaseManifestURL, body: Self.manifest(version: "1.5.0-rc1"))
        let checker = UpdateChecker(fetcher: fetcher, store: InMemoryUpdateCheckStore())

        let stable = try await checker.check()
        #expect(stable.release?.version == "1.4.0")
        #expect(!fetcher.requested.contains(UpdateChecker.prereleaseManifestURL))
    }

    @Test func offersWhicheverChannelIsNewerWhenTheSwitchIsOn() async throws {
        let fetcher = StubFetcher()
        fetcher.stub(UpdateChecker.manifestURL, body: Self.manifest(version: "1.4.0"))
        fetcher.stub(UpdateChecker.prereleaseManifestURL, body: Self.manifest(version: "1.5.0-rc1"))
        let checker = UpdateChecker(fetcher: fetcher, store: InMemoryUpdateCheckStore())
        checker.setIncludePrereleases(true)

        #expect(try await checker.check().release?.version == "1.5.0-rc1")
    }

    /// Newest wins in *both* directions: a stable release that has overtaken the pre-release
    /// channel is the answer, or the opt-in would pin testers to a stale rc.
    @Test func aStableReleaseNewerThanThePreReleaseStillWins() async throws {
        let fetcher = StubFetcher()
        fetcher.stub(UpdateChecker.manifestURL, body: Self.manifest(version: "1.5.0"))
        fetcher.stub(UpdateChecker.prereleaseManifestURL, body: Self.manifest(version: "1.5.0-rc1"))
        let checker = UpdateChecker(fetcher: fetcher, store: InMemoryUpdateCheckStore())
        checker.setIncludePrereleases(true)

        #expect(try await checker.check().release?.version == "1.5.0")
    }

    @Test func anEmptyPreReleaseChannelIsSimplyIgnored() async throws {
        let fetcher = StubFetcher()
        fetcher.stub(UpdateChecker.manifestURL, body: Self.manifest(version: "1.4.0"))
        fetcher.stub(UpdateChecker.prereleaseManifestURL, status: 404)
        let checker = UpdateChecker(fetcher: fetcher, store: InMemoryUpdateCheckStore())
        checker.setIncludePrereleases(true)

        #expect(try await checker.check().release?.version == "1.4.0")
    }

    /// #773 U5: the opt-in channel must not be able to take down the stable check. A rider who once
    /// flipped the dev switch would otherwise lose the launch sheet and the background check
    /// entirely the day the pre-release manifest 500s or goes malformed.
    @Test func aBrokenPreReleaseChannelLeavesTheStableAnswerStanding() async throws {
        let fetcher = StubFetcher()
        fetcher.stub(UpdateChecker.manifestURL, body: Self.manifest(version: "1.4.0"))
        fetcher.stub(UpdateChecker.prereleaseManifestURL, status: 500)
        let checker = UpdateChecker(fetcher: fetcher, store: InMemoryUpdateCheckStore())
        checker.setIncludePrereleases(true)

        #expect(try await checker.check().release?.version == "1.4.0")

        let garbage = StubFetcher()
        garbage.stub(UpdateChecker.manifestURL, body: Self.manifest(version: "1.4.0"))
        garbage.stub(UpdateChecker.prereleaseManifestURL, body: Data("not json".utf8))
        let second = UpdateChecker(fetcher: garbage, store: InMemoryUpdateCheckStore())
        second.setIncludePrereleases(true)

        #expect(try await second.check().release?.version == "1.4.0")
    }

    /// The *stable* channel stays loud, which is the half every rider is on: a malformed manifest
    /// there is a publishing failure and hiding it would hide it forever (U4's rule, unchanged).
    @Test func aBrokenStableChannelStillThrowsWithThePreReleaseSwitchOn() async {
        let fetcher = StubFetcher()
        fetcher.stub(UpdateChecker.manifestURL, body: Data("not json".utf8))
        fetcher.stub(UpdateChecker.prereleaseManifestURL, body: Self.manifest(version: "1.5.0-rc1"))
        let checker = UpdateChecker(fetcher: fetcher, store: InMemoryUpdateCheckStore())
        checker.setIncludePrereleases(true)

        await #expect(throws: FirmwareManifestError.notJSON) { try await checker.check() }
    }

    @Test func aPreReleaseOnlyChannelIsTheAnswerWhenNothingStableIsPublished() async throws {
        let fetcher = StubFetcher()
        fetcher.stub(UpdateChecker.manifestURL, status: 404)
        fetcher.stub(UpdateChecker.prereleaseManifestURL, body: Self.manifest(version: "1.5.0-rc1"))
        let checker = UpdateChecker(fetcher: fetcher, store: InMemoryUpdateCheckStore())
        checker.setIncludePrereleases(true)

        #expect(try await checker.check().release?.version == "1.5.0-rc1")
    }

    @Test func thePreReleaseSwitchPersistsThroughTheStore() {
        let store = InMemoryUpdateCheckStore()
        let checker = UpdateChecker(fetcher: StubFetcher(), store: store)
        #expect(!checker.includePrereleases)
        checker.setIncludePrereleases(true)
        #expect(checker.includePrereleases)
        #expect(store.loadIncludePrereleases())
    }

    // MARK: The download

    @Test func returnsAContainerThatMatchesTheManifest() async throws {
        let body = Data("abc".utf8)
        let release = FirmwareRelease(
            version: "1.4.0",
            bytes: body.count,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            url: URL(string: "https://updates.openbikecomputer.com/fw/v1.4.0/UPDATE.BIN")!
        )
        let fetcher = StubFetcher()
        fetcher.stub(release.url, body: body)
        let got = try await UpdateChecker(fetcher: fetcher, store: InMemoryUpdateCheckStore()).download(release)
        #expect(got == body)
    }

    /// The whole point of verifying on the phone: a download that doesn't match the manifest is
    /// thrown away here, so nothing wrong ever reaches the device.
    @Test func refusesADownloadThatDoesNotMatchTheManifest() async throws {
        let body = Data("abc".utf8)
        let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        let url = URL(string: "https://updates.openbikecomputer.com/fw/v1.4.0/UPDATE.BIN")!
        let fetcher = StubFetcher()
        fetcher.stub(url, body: body)
        let checker = UpdateChecker(fetcher: fetcher, store: InMemoryUpdateCheckStore())

        // Wrong length — caught before the digest is even computed.
        await #expect(throws: FirmwareDownloadError.sizeMismatch(expected: 4, got: 3)) {
            _ = try await checker.download(
                FirmwareRelease(version: "1.4.0", bytes: 4, sha256: digest, url: url)
            )
        }
        // Right length, wrong bytes.
        await #expect(throws: FirmwareDownloadError.digestMismatch) {
            _ = try await checker.download(
                FirmwareRelease(version: "1.4.0", bytes: 3, sha256: String(repeating: "f", count: 64), url: url)
            )
        }
        // The container isn't there at all.
        fetcher.stub(url, status: 404)
        await #expect(throws: FirmwareDownloadError.httpStatus(404)) {
            _ = try await checker.download(
                FirmwareRelease(version: "1.4.0", bytes: 3, sha256: digest, url: url)
            )
        }
    }

    @Test func speaksTheDigestDialectTheManifestUses() {
        // The classic vector, lowercase hex — the manifest's spelling.
        #expect(
            UpdateChecker.sha256Hex(Data("abc".utf8))
                == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        )
    }
}

/// A `ManifestFetching` that answers from a table and remembers what was asked for. Anything not
/// stubbed answers 404 — the "nothing published" default, which is also what the real server says
/// until #773's U3 ships.
private final class StubFetcher: ManifestFetching, @unchecked Sendable {
    private let lock = NSLock()
    private var responses: [URL: (Int, Data)] = [:]
    private var asked: [URL] = []

    var requested: [URL] { lock.withLock { asked } }

    func stub(_ url: URL, status: Int = 200, body: Data = Data()) {
        lock.withLock { responses[url] = (status, body) }
    }

    func get(_ url: URL) async throws -> (status: Int, body: Data) {
        lock.withLock {
            asked.append(url)
            return responses[url] ?? (404, Data())
        }
    }
}
