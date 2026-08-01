import Foundation

/// "Is there a newer firmware than the one running?" — the published manifest, and the version
/// dialect the comparison needs.
///
/// This is the **Swift twin of the builder's `release.ts`** (`builder/app/src/lib/firmware/
/// release.ts`), deliberately field-for-field and rule-for-rule: the two parsers read the same
/// manifest and must never drift, so this file's test suite is a port of that module's test matrix.
/// Anything changed here has to change there, and vice versa.
///
/// #773 locks the distribution end and U4 does not relitigate it: the manifest is fetched with an
/// **anonymous GET** — no accounts, no headers worth naming, and nothing about the device is sent.
/// The app reads that manifest and compares it against the running version; it never decides what
/// an update *is*.
///
/// ## The version dialect, which is the part with teeth
///
/// A firmware revision string comes from the Device Information Service (0x2A26). Today that is
/// `CARGO_PKG_VERSION+git-hash`; after #773's U1 it prefers the *installed* OBCU container's
/// version. Either way some devices report something that is not a release version at all — a
/// probe-flashed dev build reports a bare hash — and #773 states the consequence plainly: the app
/// **cannot parse it as a version and never offers an auto-update**. That is a locked behaviour,
/// not a limitation to work around, so ``FirmwareVersion/compare(_:_:)`` refuses rather than
/// guesses and ``FirmwareVersion/updateStatus(running:latest:)`` answers ``FirmwareUpdateStatus/unknown``.

/// What the manifest says about the newest published build.
///
/// `Codable` because the last answer is cached (``UpdateCheckRecord``) so the screen has something
/// to show before the network does.
public struct FirmwareRelease: Equatable, Sendable, Codable {
    /// The release version, as tagged (`1.4.0`, `v1.4.0`).
    public let version: String
    /// Bytes of the `UPDATE.BIN` container.
    public let bytes: Int
    /// Lowercase hex SHA-256 of the container.
    public let sha256: String
    /// Where the container is fetched from (https only).
    public let url: URL
    /// Release notes, if the manifest points at any — a URL in practice, kept as the manifest's
    /// own string so a non-URL value round-trips instead of being silently dropped.
    public let notes: String?

    public init(version: String, bytes: Int, sha256: String, url: URL, notes: String? = nil) {
        self.version = version
        self.bytes = bytes
        self.sha256 = sha256
        self.url = url
        self.notes = notes
    }

    /// The notes link, when the manifest's `notes` is one the app can open.
    public var notesURL: URL? {
        guard let notes, notes.hasPrefix("https://") else { return nil }
        return URL(string: notes)
    }
}

/// Why a manifest isn't usable. Each maps to one plain sentence in the update section — and every
/// one of them is *loud*: a half-understood manifest that offers a download is worse than no
/// manifest. A 404 is not in here, because "nothing published yet" is not an error.
public enum FirmwareManifestError: Error, Equatable, Sendable {
    /// The body isn't JSON at all.
    case notJSON
    /// The body is JSON but not an object (an array, a bare number…).
    case notAnObject
    /// A required string field is missing or empty.
    case missingField(String)
    /// `version` is present but isn't a release version (see ``FirmwareVersion``).
    case notAReleaseVersion(String)
    /// `bytes`/`size` is missing or isn't a positive integer.
    case badSize
    /// `sha256` isn't a 64-character hex digest.
    case badDigest
    /// `url` isn't an https URL.
    case insecureURL
    /// The manifest couldn't be fetched (any non-2xx that isn't a 404).
    case httpStatus(Int)
}

/// Parse a whole manifest body.
///
/// Whole, not streamed, and every required field checked before any of it is used. Unknown fields
/// (`signature`, whatever U3 adds later) are ignored — this parser reads only what the check needs.
public func parseFirmwareManifest(_ body: Data) throws -> FirmwareRelease {
    let json: Any
    do {
        json = try JSONSerialization.jsonObject(with: body, options: [])
    } catch {
        throw FirmwareManifestError.notJSON
    }
    guard let raw = json as? [String: Any] else { throw FirmwareManifestError.notAnObject }

    let version = try string(raw, "version")
    guard FirmwareVersion.parse(version) != nil else {
        throw FirmwareManifestError.notAReleaseVersion(version)
    }
    guard let bytes = positiveInteger(raw["bytes"] ?? raw["size"]) else {
        throw FirmwareManifestError.badSize
    }
    let sha256 = try string(raw, "sha256").lowercased()
    guard sha256.count == 64, sha256.allSatisfy({ $0.isHexDigitLowercase }) else {
        throw FirmwareManifestError.badDigest
    }
    let urlText = try string(raw, "url")
    guard urlText.hasPrefix("https://"), let url = URL(string: urlText) else {
        throw FirmwareManifestError.insecureURL
    }
    let notes = (raw["notes"] as? String).flatMap { $0.isEmpty ? nil : $0 }
    return FirmwareRelease(version: version, bytes: bytes, sha256: sha256, url: url, notes: notes)
}

private func string(_ raw: [String: Any], _ key: String) throws -> String {
    guard let value = raw[key] as? String, !value.isEmpty else {
        throw FirmwareManifestError.missingField(key)
    }
    return value
}

/// A JSON number that is a positive integer — `true` (which bridges to an `NSNumber`) and `1.5`
/// are both rejected, mirroring the TS parser's `Number.isInteger` guard.
private func positiveInteger(_ value: Any?) -> Int? {
    guard let number = value as? NSNumber else { return nil }
    guard CFGetTypeID(number) != CFBooleanGetTypeID() else { return nil }
    let double = number.doubleValue
    // Keep this identical to JavaScript's `Number.isSafeInteger`: the same manifest must have one
    // meaning in both clients, and converting a rounded value near `Int.max` can itself trap.
    guard double > 0, double <= 9_007_199_254_740_991, double.rounded() == double else { return nil }
    return Int(double)
}

extension Character {
    fileprivate var isHexDigitLowercase: Bool {
        ("0"..."9").contains(self) || ("a"..."f").contains(self)
    }
}

// MARK: - The version dialect

/// A release version: `v?major.minor.patch[-pre][+build]`, with `+build` **ignored**.
///
/// A straight port of the builder's `parseVersion`/`compareVersions`, down to the pre-release
/// ordering, so `1.2.0+abc1234` (what DIS reports today) and `1.2.0` are the same version and a
/// bare git hash parses as nothing at all — which is the point.
public struct FirmwareVersion: Equatable, Sendable {
    public let major: Int
    public let minor: Int
    public let patch: Int
    /// A pre-release tag (`rc1`), which sorts *before* the same triple without one.
    public let pre: String?

    public init(major: Int, minor: Int, patch: Int, pre: String? = nil) {
        self.major = major
        self.minor = minor
        self.patch = patch
        self.pre = pre
    }

    /// Parse a release version, or `nil` for anything that is not one.
    ///
    /// The dialect is the builder's regex, hand-rolled: an optional leading `v`, a three-part
    /// numeric core, an optional `-pre` tag of `[0-9A-Za-z.-]`, and optional `+build` metadata of
    /// the same alphabet which is parsed only to be discarded. A numeric part outside JavaScript's
    /// safe-integer range is treated as unparseable so both clients give it exactly one meaning.
    public static func parse(_ text: String) -> FirmwareVersion? {
        var rest = Substring(text.trimmingCharacters(in: .whitespacesAndNewlines))
        if rest.first == "v" { rest = rest.dropFirst() }

        // `+build` first: the build alphabet contains no `+`, so the first one delimits it.
        if let plus = rest.firstIndex(of: "+") {
            let build = rest[rest.index(after: plus)...]
            guard !build.isEmpty, build.allSatisfy(isVersionTagCharacter) else { return nil }
            rest = rest[..<plus]
        }
        // Then `-pre`: the numeric core contains no `-`, so the first one delimits it.
        var pre: String?
        if let dash = rest.firstIndex(of: "-") {
            let tag = rest[rest.index(after: dash)...]
            guard !tag.isEmpty, tag.allSatisfy(isVersionTagCharacter) else { return nil }
            pre = String(tag)
            rest = rest[..<dash]
        }

        let parts = rest.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 3 else { return nil }
        var numbers: [Int] = []
        for part in parts {
            guard
                !part.isEmpty,
                part.allSatisfy({ $0.isASCII && $0.isNumber }),
                let n = Int(part),
                n <= 9_007_199_254_740_991
            else {
                return nil
            }
            numbers.append(n)
        }
        return FirmwareVersion(major: numbers[0], minor: numbers[1], patch: numbers[2], pre: pre)
    }

    /// Order two version strings: negative if `a` is older, 0 if equal, positive if newer.
    ///
    /// `nil` when either side is not a release version. Callers must treat that as "cannot say"
    /// rather than as "not newer" — #773's rule is that an unparseable running version means no
    /// update is ever offered, and collapsing `nil` into `0` would silently offer one.
    public static func compare(_ a: String, _ b: String) -> Int? {
        guard let left = parse(a), let right = parse(b) else { return nil }
        // Do not subtract untrusted numeric input just to learn its order. Relational comparison
        // states the actual intent and stays safe if the accepted component range ever changes.
        if left.major != right.major { return left.major < right.major ? -1 : 1 }
        if left.minor != right.minor { return left.minor < right.minor ? -1 : 1 }
        if left.patch != right.patch { return left.patch < right.patch ? -1 : 1 }
        if left.pre == right.pre { return 0 }
        // A pre-release precedes its release; between two pre-releases, plain lexicographic order
        // over the bytes is enough for the one thing this decides ("is the published one newer
        // than mine") — and matching the builder's JS string comparison is what keeps the two
        // parsers from drifting on `rc1` vs `rc2`.
        if left.pre == nil { return 1 }
        if right.pre == nil { return -1 }
        return left.pre!.utf8.lexicographicallyPrecedes(right.pre!.utf8) ? -1 : 1
    }

    /// What to say about a device running `running` when the newest published build is `latest`.
    ///
    /// An unparseable running version answers ``FirmwareUpdateStatus/unknown`` **even when nothing
    /// is published**, and that ordering is deliberate (it matches the builder's, PR #1004): what
    /// makes a dev build undecidable is the version it reports, not the absence of a manifest.
    /// Answering `noRelease` there would hide the "development build — automatic updates are
    /// paused" line behind whichever publication happens to exist that day — i.e. hide it
    /// entirely until U3 first publishes.
    ///
    /// A device that has said *nothing* yet is a different thing from a dev build: no running
    /// version and no release is `noRelease` (there is simply no check to make), and no running
    /// version against a published release is `unknown` (nothing to compare against yet).
    public static func updateStatus(running: String?, latest: String?) -> FirmwareUpdateStatus {
        if let running, !running.isEmpty, parse(running) == nil { return .unknown }
        guard let latest, !latest.isEmpty else { return .noRelease }
        guard let running, !running.isEmpty else { return .unknown }
        guard let order = compare(running, latest) else { return .unknown }
        if order < 0 { return .available }
        return order > 0 ? .ahead : .current
    }

    private static func isVersionTagCharacter(_ c: Character) -> Bool {
        guard c.isASCII else { return false }
        return c.isNumber || c.isLetter || c == "." || c == "-"
    }
}

/// The five answers the update check can give.
public enum FirmwareUpdateStatus: Equatable, Sendable {
    /// Nothing is published yet (the manifest 404s) — say nothing loud.
    case noRelease
    /// The running version is not a release version (a probe-flashed dev build). No update is
    /// offered; #773 locks that.
    case unknown
    /// The device is on the newest published build.
    case current
    /// A newer build is published.
    case available
    /// The device is running something newer than what is published. Says so; never offers a
    /// downgrade.
    case ahead
}
