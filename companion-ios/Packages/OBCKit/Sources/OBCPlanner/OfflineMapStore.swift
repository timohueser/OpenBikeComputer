import CryptoKit
import Darwin
import Foundation
import zlib

public actor OfflineMapStore {
    public let root: URL
    private let api: URL
    private let metadataSession: URLSession
    private let capacity: (@Sendable (URL) throws -> Int64)?
    private var installing = false

    public init(root: URL, api: URL = URL(string: "https://releases.openbikecomputer.com/planner-offline/")!,
                session: URLSession = .shared, capacity: (@Sendable (URL) throws -> Int64)? = nil) {
        self.root = root; self.api = api; self.metadataSession = session; self.capacity = capacity
    }

    public func maps() throws -> [OfflineMap] {
        let path = root.appending(path: "maps.json")
        guard FileManager.default.fileExists(atPath: path.path) else { return [] }
        let maps = try JSONDecoder().decode([OfflineMap].self, from: Data(contentsOf: path))
        guard maps.allSatisfy({ OfflineFile.validHash($0.id) && OfflineMap.valid($0.bounds) && $0.installedBytes >= 0 }),
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
        return OfflineDownloadQuote(map: quote.map,
            transferBytes: try sum(quote.bundle.objects.filter { !hasObject($0) }.map(\.transport.bytes)),
            requiredBytes: try needed(quote.bundle), source: quote.source, bundle: quote.bundle, release: quote.release)
    }

    public func discardPending() throws {
        guard !installing else { throw OfflineMapFailure.unavailable("Pause the download first.") }
        let path = root.appending(path: "pending.json")
        if FileManager.default.fileExists(atPath: path.path) { try FileManager.default.removeItem(at: path) }
        let downloads = root.appending(path: "downloads")
        if FileManager.default.fileExists(atPath: downloads.path) { try FileManager.default.removeItem(at: downloads) }
        try collectObjects()
    }

    public func coverage() async throws -> OfflineCoverage {
        let coverage = try JSONDecoder().decode(OfflineCoverage.self, from: await get(api.appending(path: "catalog")))
        guard coverage.format == 1, coverage.zoom == 9, OfflineMap.valid(coverage.bounds) else { throw PlannerFailure.invalidData }
        return coverage
    }

    public func prepare(bounds: [Double], name: String,
                        status: @escaping @Sendable (Double, String) -> Void) async throws -> OfflineDownloadQuote {
        guard OfflineMap.valid(bounds) else { throw PlannerFailure.invalidData }
        var request = URLRequest(url: api.appending(path: "jobs"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: ["bounds": bounds])
        status(0.1, "Checking map coverage")
        struct Job: Decodable { let id: String; let state: String; let message: String?; let source: URL }
        let job = try JSONDecoder().decode(Job.self, from: await send(request))
        guard OfflineFile.validHash(job.id), job.state == "ready",
              job.source.scheme == "https", job.source.host != nil,
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
        let installed = try sum(bundle.objects.map(\.bytes) + [Int64(release.count)])
        let transfer = try sum(bundle.objects.filter { !hasObject($0) }.map(\.transport.bytes))
        let map = OfflineMap(id: bundle.release.sha256, name: name, region: manifest.region,
                             bounds: manifest.bounds, installedBytes: installed)
        status(1, "Ready to download")
        return OfflineDownloadQuote(map: map, transferBytes: transfer,
                                    requiredBytes: try needed(bundle), source: source, bundle: bundle, release: release)
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
        try checkSpace(needed(quote.bundle))
        try JSONEncoder().encode(quote).write(to: root.appending(path: "pending.json"), options: .atomic)
        let total = try sum(quote.bundle.objects.filter { !hasObject($0) }.map(\.transport.bytes))
        let configuration = metadataSession.configuration
        configuration.urlCache = nil
        configuration.allowsCellularAccess = allowMobileData
        configuration.allowsExpensiveNetworkAccess = allowMobileData
        configuration.allowsConstrainedNetworkAccess = allowMobileData
        configuration.waitsForConnectivity = true
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let transfer = OfflineTransfer(root: root, source: quote.source.appending(path: "objects"),
            total: total, session: session, progress: progress)
        let missing = quote.bundle.objects.filter { !hasObject($0) }
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
        var maps = try maps().filter { $0.id != quote.map.id }
        maps.append(quote.map)
        try JSONEncoder().encode(maps).write(to: root.appending(path: "maps.json"), options: .atomic)
        try fm.removeItem(at: root.appending(path: "pending.json"))
        progress(total, total, "Ready offline")
    }

    public func remove(_ map: OfflineMap) throws {
        guard !installing else { throw OfflineMapFailure.unavailable("Stop the download before removing a map.") }
        let maps = try maps().filter { $0.id != map.id }
        try JSONEncoder().encode(maps).write(to: root.appending(path: "maps.json"), options: .atomic)
        let directory = root.appending(path: "releases/\(map.id)")
        if FileManager.default.fileExists(atPath: directory.path) { try FileManager.default.removeItem(at: directory) }
        try collectObjects()
    }

    private func collectObjects() throws {
        var retained = Set(try pending()?.bundle.objects.map(\.sha256) ?? [])
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

    private func needed(_ bundle: OfflineBundle) throws -> Int64 {
        let missing = bundle.objects.filter { !hasObject($0) }
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
        return try sum(try missing.map { try allocated($0.bytes) } + [
            sum(missing.filter { $0.transport.encoding == "gzip" }.map(\.transport.bytes).sorted(by: >).prefix(4).map { try allocated($0) }),
            allocated(Int64(metadata) * 2), block * Int64(directories.count + bundle.files.count + 4)])
    }

    /// An object is named by its content hash and moves into `objects/` only after `OfflineTransfer`
    /// verifies it, so name and size identify a complete object.
    private func hasObject(_ entry: OfflineBundle.Entry) -> Bool {
        let path = root.appending(path: "objects/\(entry.sha256)")
        return (try? path.resourceValues(forKeys: [.fileSizeKey]).fileSize).map { Int64($0) == entry.bytes } ?? false
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
