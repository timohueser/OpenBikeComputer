import CryptoKit
import Darwin
import Foundation
import zlib

public actor OfflineMapStore {
    public let root: URL
    private let planner: PlannerService
    private let installer: any OfflineRouting
    private let metadataSession: URLSession
    private let capacity: (@Sendable (URL) throws -> Int64)?
    private var installing = false

    public init(root: URL, routing: any OfflineRouting, planner: PlannerService = .shared,
                session: URLSession = .shared, capacity: (@Sendable (URL) throws -> Int64)? = nil) {
        self.root = root; self.installer = routing; self.planner = planner; self.metadataSession = session
        self.capacity = capacity
    }

    /// The routing install that `map` shares with each map of its routing package.
    public nonisolated func routing(_ map: OfflineMap) -> URL { root.appending(path: "routing/\(map.routing)") }

    /// The downloads service of the active planner release, or the shared one when the catalogue names none.
    private func service() async throws -> URL {
        try await planner.release().downloads ?? URL(string: "https://releases.openbikecomputer.com/planner-offline/")!
    }

    public func maps() throws -> [OfflineMap] {
        let path = root.appending(path: "maps.json")
        guard FileManager.default.fileExists(atPath: path.path) else { return [] }
        let maps = try JSONDecoder().decode([OfflineMap].self, from: Data(contentsOf: path))
        guard maps.allSatisfy({ OfflineFile.validHash($0.id) && OfflineFile.validHash($0.routing)
                  && OfflineMap.valid($0.bounds) && $0.installedBytes >= 0 }),
              Set(maps.map(\.id)).count == maps.count else { throw PlannerFailure.invalidData }
        return maps
    }

    public func availableBytes() throws -> Int64 {
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        var directory = root, values = URLResourceValues()
        values.isExcludedFromBackup = true
        try directory.setResourceValues(values)
        if let capacity { return try capacity(root) }
        guard let bytes = try root.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey])
            .volumeAvailableCapacityForImportantUsage else {
            throw OfflineMapFailure.unavailable("Free space could not be checked. Try again.")
        }
        return bytes
    }

    public func pending() throws -> OfflineDownloadQuote? {
        let file = root.appending(path: "pending.json")
        guard FileManager.default.fileExists(atPath: file.path) else { return nil }
        let quote = try JSONDecoder().decode(OfflineDownloadQuote.self, from: Data(contentsOf: file))
        _ = try quote.bundle.validate(release: quote.release)
        guard quote.map.id == quote.bundle.release.sha256 else { throw PlannerFailure.invalidData }
        return OfflineDownloadQuote(map: quote.map, transferBytes: try missingBytes(quote.bundle, quote.routing),
            requiredBytes: try needed(quote.bundle, quote.routing), source: quote.source, bundle: quote.bundle,
            release: quote.release, routing: quote.routing)
    }

    public func discardPending() async throws {
        guard !installing else { throw OfflineMapFailure.unavailable("Pause the download first.") }
        installing = true
        defer { installing = false }
        // A stopped install can leave cells of the pending map in its routing folder. With no listed map of
        // the package the folder goes, also when it is only partly written; else the listed maps install
        // again from the copies in the folder, which their complete install wrote.
        if let pending = try? pending() {
            let target = routing(pending.map)
            let areas = try maps().filter { $0.routing == pending.map.routing }.map(\.bounds)
            if areas.isEmpty {
                if FileManager.default.fileExists(atPath: target.path) { try FileManager.default.removeItem(at: target) }
            } else {
                try await installer.install(package: target.appending(path: "package.json"),
                    directory: target.appending(path: "directory.bin"), target: target, areas: areas,
                    objects: root.appending(path: "objects")) { _, _ in }
            }
        }
        let path = root.appending(path: "pending.json")
        if FileManager.default.fileExists(atPath: path.path) { try FileManager.default.removeItem(at: path) }
        let downloads = root.appending(path: "downloads")
        if FileManager.default.fileExists(atPath: downloads.path) { try FileManager.default.removeItem(at: downloads) }
        try collectObjects()
    }

    public func coverage() async throws -> OfflineCoverage {
        let coverage = try JSONDecoder().decode(OfflineCoverage.self, from: await get(service().appending(path: "catalog")))
        guard coverage.format == 2, coverage.zoom == 9, OfflineMap.valid(coverage.bounds) else { throw PlannerFailure.invalidData }
        return coverage
    }

    public func prepare(bounds: [Double], name: String,
                        status: @escaping @Sendable (Double, String) -> Void) async throws -> OfflineDownloadQuote {
        guard OfflineMap.valid(bounds) else { throw PlannerFailure.invalidData }
        let entry = try await service()
        var request = URLRequest(url: entry.appending(path: "jobs"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: ["bounds": bounds])
        status(0.1, "Checking map coverage")
        struct Job: Decodable { let id: String; let state: String; let message: String?; let source: URL }
        let job = try JSONDecoder().decode(Job.self, from: await send(request))
        guard OfflineFile.validHash(job.id), job.state == "ready",
              job.source.scheme == entry.scheme, job.source.host != nil,
              job.source.user == nil, job.source.password == nil,
              job.source.query == nil, job.source.fragment == nil,
              Array(job.source.pathComponents.suffix(2)) == ["bundles", job.id] else {
            throw OfflineMapFailure.unavailable(job.message ?? "The map size could not be checked. Try again.")
        }
        status(0.6, "Reading download size")
        let source = job.source
        let bytes = try await get(source.appending(path: "bundle.json"))
        let bundle = try JSONDecoder().decode(OfflineBundle.self, from: bytes)
        let release = try await get(source.appending(path: "release.json"))
        let manifest = try bundle.validate(release: release)
        status(0.8, "Loading routes…")
        try await routingFiles(bundle, source: source, session: metadataSession)
        let own = try await installer.plan(package: object(bundle, OfflineManifest.routingPackage), directory: object(bundle, OfflineManifest.routingDirectory),
            target: root.appending(path: "routing/\(manifest.routing_package)"), areas: [manifest.bounds])
        let map = OfflineMap(id: bundle.release.sha256, name: name, region: manifest.region, bounds: manifest.bounds,
            routing: manifest.routing_package,
            installedBytes: try sum(bundle.objects.map(\.bytes) + [Int64(release.count), own.bytes]))
        let objects = try await plan(map, bundle).objects
        status(1, "Ready to download")
        return OfflineDownloadQuote(map: map, transferBytes: try missingBytes(bundle, objects),
            requiredBytes: try needed(bundle, objects), source: source, bundle: bundle, release: release, routing: objects)
    }

    /// The objects of the routing files of `bundle`, which `routingFiles` fetched.
    private func object(_ bundle: OfflineBundle, _ path: String) throws -> URL {
        guard let entry = bundle.files[path] else { throw PlannerFailure.invalidData }
        return root.appending(path: "objects/\(entry.sha256)")
    }

    /// Fetches the routing files of `bundle` into `objects/`, through the path of the large files: a
    /// world directory is larger than the limit of `get`.
    private func routingFiles(_ bundle: OfflineBundle, source: URL, session: URLSession) async throws {
        for name in ["objects", "downloads"] {
            try FileManager.default.createDirectory(at: root.appending(path: name), withIntermediateDirectories: true)
        }
        let entries = try [OfflineManifest.routingPackage, OfflineManifest.routingDirectory].map { path in
            guard let entry = bundle.files[path] else { throw PlannerFailure.invalidData }
            return entry
        }.filter { !hasObject($0) }
        let transfer = OfflineTransfer(root: root, source: source.appending(path: "objects"),
            total: try sum(entries.map(\.transport.bytes)), session: session) { _, _, _ in }
        for entry in entries { try await transfer.install(entry) }
    }

    /// The routing plan of the union of the areas of the maps of the routing package of `map`, with `map`.
    private func plan(_ map: OfflineMap, _ bundle: OfflineBundle) async throws -> OfflineRoutingPlan {
        let areas = try maps().filter { $0.routing == map.routing && $0.id != map.id }.map(\.bounds) + [map.bounds]
        return try await installer.plan(package: object(bundle, OfflineManifest.routingPackage), directory: object(bundle, OfflineManifest.routingDirectory),
                                        target: routing(map), areas: areas)
    }

    /// An entry of `OfflineTransfer` for a routing object, which the release pool serves as it is.
    private static func entry(_ file: OfflineFile) -> OfflineBundle.Entry {
        .init(bytes: file.bytes, sha256: file.sha256,
              transport: .init(bytes: file.bytes, sha256: file.sha256, encoding: "identity"))
    }

    public func install(_ quote: OfflineDownloadQuote, allowMobileData: Bool,
                        progress: @escaping @Sendable (Int64, Int64, String) -> Void) async throws {
        guard !installing else { throw OfflineMapFailure.unavailable("Another map is downloading.") }
        installing = true
        defer { installing = false }
        _ = try quote.bundle.validate(release: quote.release)
        let fm = FileManager.default
        for name in ["objects", "downloads", "releases"] {
            try fm.createDirectory(at: root.appending(path: name), withIntermediateDirectories: true)
        }
        try checkSpace(needed(quote.bundle, quote.routing))
        let configuration = metadataSession.configuration
        configuration.urlCache = nil
        configuration.allowsCellularAccess = allowMobileData
        configuration.allowsExpensiveNetworkAccess = allowMobileData
        configuration.allowsConstrainedNetworkAccess = allowMobileData
        configuration.waitsForConnectivity = true
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        // The maps can change after `prepare`, so the install plans the union again.
        try await routingFiles(quote.bundle, source: quote.source, session: session)
        let objects = try await plan(quote.map, quote.bundle).objects
        try checkSpace(needed(quote.bundle, objects))
        let pending = OfflineDownloadQuote(map: quote.map, transferBytes: quote.transferBytes,
            requiredBytes: quote.requiredBytes, source: quote.source, bundle: quote.bundle, release: quote.release,
            routing: objects)
        try JSONEncoder().encode(pending).write(to: root.appending(path: "pending.json"), options: .atomic)
        let total = try missingBytes(quote.bundle, objects)
        let transfer = OfflineTransfer(root: root, source: quote.source.appending(path: "objects"),
            total: total, session: session, progress: progress)
        let missing = quote.bundle.objects.filter { !hasObject($0) } + unique(objects).filter { !hasObject($0) }.map { Self.entry($0) }
        try await withThrowingTaskGroup(of: Void.self) { group in
            var next = missing.makeIterator()
            for _ in 0..<4 {
                if let entry = next.next() { group.addTask { try await transfer.install(entry) } }
            }
            while try await group.next() != nil {
                if let entry = next.next() { group.addTask { try await transfer.install(entry) } }
            }
        }
        try Task.checkCancellation()
        // The routing installs from `objects/` before the release folder is linked, so the link and
        // `maps.json` follow it with no suspension between them, and a stopped install links nothing.
        let areas = try maps().filter { $0.routing == quote.map.routing && $0.id != quote.map.id }.map(\.bounds)
        try await installer.install(package: object(quote.bundle, OfflineManifest.routingPackage),
            directory: object(quote.bundle, OfflineManifest.routingDirectory), target: routing(quote.map),
            areas: areas + [quote.map.bounds], objects: root.appending(path: "objects")) { done, all in
            progress(done, all, "Installing routes…")
        }
        let directory = root.appending(path: "releases/\(quote.map.id)")
        let stage = root.appending(path: "releases/.\(quote.map.id)")
        if fm.fileExists(atPath: stage.path) { try fm.removeItem(at: stage) }
        try fm.createDirectory(at: stage, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: stage) }
        for (name, entry) in quote.bundle.files {
            let destination = stage.appending(path: name)
            try fm.createDirectory(at: destination.deletingLastPathComponent(), withIntermediateDirectories: true)
            try fm.linkItem(at: root.appending(path: "objects/\(entry.sha256)"), to: destination)
        }
        try quote.release.write(to: stage.appending(path: "release.json"), options: .atomic)
        if fm.fileExists(atPath: directory.path) {
            guard renameatx_np(AT_FDCWD, stage.path, AT_FDCWD, directory.path, UInt32(RENAME_SWAP)) == 0 else {
                throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
            }
        } else { try fm.moveItem(at: stage, to: directory) }
        var updated = try maps().filter { $0.id != quote.map.id }
        updated.append(quote.map)
        try JSONEncoder().encode(updated).write(to: root.appending(path: "maps.json"), options: .atomic)
        try fm.removeItem(at: root.appending(path: "pending.json"))
        try collectObjects()
        progress(total, total, "Ready offline")
    }

    public func remove(_ map: OfflineMap) async throws {
        guard !installing else { throw OfflineMapFailure.unavailable("Stop the download before removing a map.") }
        installing = true
        defer { installing = false }
        let directory = root.appending(path: "releases/\(map.id)")
        // The install comes first: a route during the `await` must not cache a reader under the new map list
        // on the old `install.bin`.
        let others = try maps().filter { $0.id != map.id }
        let remaining = others.filter { $0.routing == map.routing }.map(\.bounds)
        do {
            try await installer.install(package: directory.appending(path: OfflineManifest.routingPackage),
                directory: directory.appending(path: OfflineManifest.routingDirectory), target: routing(map),
                areas: remaining, objects: root.appending(path: "objects")) { _, _ in }
        } catch PlannerFailure.invalidData where !remaining.isEmpty {
            // The cells of the remaining maps can be gone after a removal that stopped before `maps.json`;
            // the folder stays for them, and their routes that miss a cell go online.
        }
        try JSONEncoder().encode(others).write(to: root.appending(path: "maps.json"), options: .atomic)
        if FileManager.default.fileExists(atPath: directory.path) { try FileManager.default.removeItem(at: directory) }
        try collectObjects()
    }

    /// Keeps the objects of the pending download and of each map. Routing objects are in no `release.json`,
    /// so they go after each install.
    private func collectObjects() throws {
        let pending = try pending()
        var retained = Set((pending?.bundle.objects.map(\.sha256) ?? []) + (pending?.routing.map(\.sha256) ?? []))
        for map in try maps() {
            let data = try Data(contentsOf: root.appending(path: "releases/\(map.id)/release.json"))
            let manifest = try JSONDecoder().decode(OfflineManifest.self, from: data)
            retained.formUnion(manifest.files.values.map(\.sha256))
        }
        let objects = root.appending(path: "objects")
        guard FileManager.default.fileExists(atPath: objects.path) else { return }
        for file in try FileManager.default.contentsOfDirectory(at: objects, includingPropertiesForKeys: nil)
            where !retained.contains(file.lastPathComponent) {
            try FileManager.default.removeItem(at: file)
        }
    }

    /// The transport bytes of the missing objects of `bundle` and of the missing routing objects.
    private func missingBytes(_ bundle: OfflineBundle, _ routing: [OfflineFile]) throws -> Int64 {
        try sum(bundle.objects.filter { !hasObject($0) }.map(\.transport.bytes) + unique(routing).filter { !hasObject($0) }.map(\.bytes))
    }

    /// The routing objects once each: parts with equal bytes share one object.
    private func unique(_ files: [OfflineFile]) -> [OfflineFile] {
        var seen: Set<String> = []
        return files.filter { seen.insert($0.sha256).inserted }
    }

    private func needed(_ bundle: OfflineBundle, _ routing: [OfflineFile]) throws -> Int64 {
        let missing = bundle.objects.filter { !hasObject($0) }
        let missingRouting = routing.filter { !hasObject($0) }
        // A new routing package also copies `package.json` and `directory.bin` into its install folder.
        let files = [OfflineManifest.routingPackage, OfflineManifest.routingDirectory].compactMap { bundle.files[$0] }
        let folder = root.appending(path: "routing/\(bundle.files[OfflineManifest.routingDirectory]?.sha256 ?? "")")
        let copies = FileManager.default.fileExists(atPath: folder.appending(path: "directory.bin").path)
            ? [] : files.map(\.bytes)
        _ = try availableBytes()
        var info = statfs()
        guard statfs(root.path, &info) == 0 else { throw POSIXError(.EIO) }
        let block = Int64(info.f_bsize)
        func allocated(_ bytes: Int64) throws -> Int64 {
            let rounded = try sum([bytes, block - 1]) / block
            let (result, overflow) = rounded.multipliedReportingOverflow(by: block)
            guard !overflow else { throw PlannerFailure.invalidData }
            return result
        }
        let directories = Set(bundle.files.keys.flatMap { path -> [String] in
            let parts = path.split(separator: "/")
            return (1..<parts.count).map { parts.prefix($0).joined(separator: "/") }
        })
        // Allocate for object tails, directory blocks, and both copies of atomically written metadata.
        let metadata = try JSONEncoder().encode(bundle).count + Int(bundle.release.bytes) * 2
        return try sum(try missing.map { try allocated($0.bytes) }
            + unique(missingRouting).map { try allocated($0.bytes) } + missingRouting.map { try allocated($0.part_bytes ?? 0) }
            + copies.map { try allocated($0) } + [
            sum(missing.filter { $0.transport.encoding == "gzip" }.map(\.transport.bytes).sorted(by: >).prefix(4).map { try allocated($0) }),
            allocated(Int64(metadata) * 2), block * Int64(directories.count + bundle.files.count + 4)])
    }

    /// An object is named by its content hash and moves into `objects/` only after `OfflineTransfer`
    /// verifies it, so name and size identify a complete object.
    private func hasObject(_ entry: OfflineBundle.Entry) -> Bool { hasObject(OfflineFile(bytes: entry.bytes, sha256: entry.sha256)) }
    private func hasObject(_ file: OfflineFile) -> Bool {
        let path = root.appending(path: "objects/\(file.sha256)")
        return (try? path.resourceValues(forKeys: [.fileSizeKey]).fileSize).map { Int64($0) == file.bytes } ?? false
    }

    private func sum(_ values: [Int64]) throws -> Int64 {
        try values.reduce(0) { result, value in
            let (sum, overflow) = result.addingReportingOverflow(value)
            guard value >= 0, !overflow else { throw PlannerFailure.invalidData }
            return sum
        }
    }

    private func checkSpace(_ needed: Int64) throws {
        let available = try availableBytes()
        guard available >= needed else { throw OfflineMapFailure.notEnoughSpace(required: needed, available: available) }
    }

    private func get(_ url: URL) async throws -> Data { try await send(URLRequest(url: url)) }
    private func send(_ request: URLRequest) async throws -> Data {
        let (data, response) = try await metadataSession.data(for: request)
        guard data.count <= 16 * 1024 * 1024, let response = response as? HTTPURLResponse,
              (200..<300).contains(response.statusCode) else {
            if let response = response as? HTTPURLResponse, response.statusCode == 404,
               request.url?.pathComponents.contains("bundles") == true {
                throw OfflineMapFailure.unavailable("This download has expired. Choose the area again to prepare a new download.")
            }
            struct Failure: Decodable { let message: String }
            let message = (try? JSONDecoder().decode(Failure.self, from: data))?.message
            throw OfflineMapFailure.unavailable(message ?? "Offline downloads are unavailable. Try again later.")
        }
        return data
    }

    static func verify(_ url: URL, _ expected: OfflineFile) throws {
        let file = try FileHandle(forReadingFrom: url)
        defer { try? file.close() }
        var hash = SHA256(), count: Int64 = 0
        while let data = try file.read(upToCount: 1 << 20), !data.isEmpty {
            try Task.checkCancellation()
            count += Int64(data.count)
            guard count <= expected.bytes else { throw PlannerFailure.invalidData }
            hash.update(data: data)
        }
        guard count == expected.bytes, hash.finalize().map({ String(format: "%02x", $0) }).joined() == expected.sha256 else {
            throw PlannerFailure.invalidData
        }
    }

    static func unpack(_ source: URL, to destination: URL, bytes: Int64) throws {
        guard let input = gzopen(source.path, "rb") else { throw PlannerFailure.invalidData }
        defer { gzclose(input) }
        FileManager.default.createFile(atPath: destination.path, contents: nil)
        let output = try FileHandle(forWritingTo: destination)
        defer { try? output.close() }
        var buffer = [UInt8](repeating: 0, count: 1 << 20), total: Int64 = 0
        while true {
            try Task.checkCancellation()
            let count = gzread(input, &buffer, UInt32(buffer.count))
            guard count >= 0 else { throw PlannerFailure.invalidData }
            if count == 0 { break }
            total += Int64(count)
            guard total <= bytes else { throw PlannerFailure.invalidData }
            try output.write(contentsOf: buffer.prefix(Int(count)))
        }
        guard total == bytes else { throw PlannerFailure.invalidData }
        try output.synchronize()
    }
}
