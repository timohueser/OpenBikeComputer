import Foundation
import Observation
import OBCDomain
import OBCTransport

/// The import flow's state machine, extracted from the composition root so the replace-vs-new and
/// rename rules run under `swift test`: a file arrives, decodes into the pending import behind the
/// cover, and a name collision detours through the update-or-add dialog before the cover opens.
///
/// Formats stay at the edges: OBCUI must not import `OBCFormats`, so the decode is injected as a
/// closure over the composition root's importer, and only the flow lives here. The bond check is
/// likewise a narrow closure, because the flow needs only the framing bit.
///
/// The view binds `pendingImport`, `collision`, `addAsNewPrompt` seeded by `newRouteName`, and
/// `importFailed`; every transition between them goes through the methods below.
@MainActor @Observable
public final class ImportFlowModel {
    // MARK: Observable state, which the view binds

    /// The decoded route behind the cover; nil closes it.
    public var pendingImport: PendingImport?
    /// A just-imported route whose name matches a saved one, which drives the update-or-add dialog.
    public var collision: ImportCollision?
    /// "Add as a new route" chosen from the collision dialog; it holds the import while the
    /// distinct-name prompt is up.
    public var addAsNewPrompt: PendingImport?
    /// The name the rename prompt starts with.
    public private(set) var newRouteName = ""
    /// The "couldn't read that file" alert.
    public var importFailed = false
    /// Several files that arrived together, behind the "Make a trip" sheet; nil closes it.
    public var pendingJoin: PendingJoin?

    /// A share of several files arrives as one URL at a time. URLs closer together than this
    /// are one share.
    private let batchWindow: Duration
    @ObservationIgnored private var batch: [URL] = []
    @ObservationIgnored private var batchTask: Task<Void, Never>?

    // MARK: Injected seams

    private let decode: (Data, String) throws -> ImportedRoute
    /// The phone-side library, read directly for the collision check; see `plannedRoute(named:)`.
    private let library: any LibraryStore
    /// Bond state at arrival picks the framing. A narrow closure, not the whole `BondStore`,
    /// because the launch flow owns the record itself.
    private let isBonded: () -> Bool
    private let lastBikeType: LastBikeTypeStore

    public init(
        decode: @escaping (Data, String) throws -> ImportedRoute,
        library: any LibraryStore,
        isBonded: @escaping () -> Bool,
        lastBikeType: LastBikeTypeStore = LastBikeTypeStore(),
        batchWindow: Duration = .milliseconds(400)
    ) {
        self.batchWindow = batchWindow
        self.decode = decode
        self.library = library
        self.isBonded = isBonded
        self.lastBikeType = lastBikeType
    }

    // MARK: Opening files

    /// One shared file URL. URLs that arrive within ``batchWindow`` of each other open together.
    public func receive(_ url: URL) {
        batch.append(url)
        guard batchTask == nil else { return }
        batchTask = Task { [weak self, batchWindow] in
            try? await Task.sleep(for: batchWindow)
            guard let self else { return }
            let urls = batch
            batch = []
            batchTask = nil
            await openFiles(at: urls)
        }
    }

    /// Picked or shared files, read as `openFile(at:)` reads one, then `open(files:)`.
    public func openFiles(at urls: [URL]) async {
        var files: [(data: Data, fileName: String)] = []
        for url in urls {
            guard let data = await Self.read(url) else {
                importFailed = true
                return
            }
            files.append((data, url.lastPathComponent))
        }
        open(files: files)
    }

    /// Route files in hand: one opens the landing; several open the "Make a trip" sheet, or the
    /// alert when one does not decode.
    public func open(files: [(data: Data, fileName: String)]) {
        guard files.count > 1 else {
            if let file = files.first { open(data: file.data, fileName: file.fileName) }
            return
        }
        var pending: [PendingImport] = []
        for file in files {
            guard let route = try? decode(file.data, file.fileName) else {
                importFailed = true
                return
            }
            pending.append(PendingImport(
                route: route, fileName: file.fileName, fileData: file.data,
                noDevicePaired: !isBonded(), bikeType: lastBikeType.value))
        }
        pendingJoin = PendingJoin(files: pending)
    }

    public func closeJoin() {
        pendingJoin = nil
    }

    // MARK: Opening a file

    /// A picked or shared file URL: read it, then run the `open(data:fileName:)` flow. The
    /// security-scoped access and the read happen off the main actor, because a share-sheet file
    /// that lives in iCloud Drive and is not downloaded yet blocks inside `Data(contentsOf:)`
    /// until the download lands, and that must not freeze the UI. An unreadable file fails the
    /// same way an undecodable one does.
    public func openFile(at url: URL) async {
        guard let data = await Self.read(url) else {
            importFailed = true
            return
        }
        open(data: data, fileName: url.lastPathComponent)
    }

    /// Route-file bytes in hand: decode, then either open the landing straight away or, when a
    /// saved route already carries this name, offer update-in-place against new.
    public func open(data: Data, fileName: String) {
        do {
            open(route: try decode(data, fileName), fileName: fileName, fileData: data)
        } catch {
            importFailed = true
        }
    }

    /// A route already in hand, such as a ride saved as a route: the same landing and the same
    /// name-collision rule as a decoded file. A nil `bikeType` takes the last one picked.
    public func open(
        route: ImportedRoute, fileName: String, fileData: Data, source: ImportSource = .file,
        bikeType: BikeType? = nil
    ) {
        let pending = PendingImport(
            route: route,
            fileName: fileName,
            fileData: fileData,
            source: source,
            noDevicePaired: !isBonded(),
            bikeType: bikeType ?? lastBikeType.value
        )
        // A route by this name is already saved, so offer update-in-place against new.
        if let existing = plannedRoute(named: route.name ?? fileName) {
            collision = ImportCollision(pending: pending, existing: existing)
        } else {
            pendingImport = pending
        }
    }

    // MARK: The collision dialog

    /// "Update the existing route": open the landing pinned to replace the saved record, whose id
    /// and device link carry through.
    public func chooseReplace() {
        guard let collision else { return }
        pendingImport = collision.pending.replacing(collision.existing)
        self.collision = nil
    }

    /// "Add as a new route": two routes under one name would be indistinguishable, and the next
    /// import's collision check keys on the name, so a distinct name is required before the
    /// landing opens. This detours through the rename prompt.
    public func chooseAddAsNew() {
        guard let collision else { return }
        newRouteName = collision.pending.route.name ?? collision.pending.fileName
        addAsNewPrompt = collision.pending
        self.collision = nil
    }

    public func cancelCollision() {
        collision = nil
    }

    // MARK: The "Add as a new route" prompt

    /// Whether the prompt can accept `name`: non-empty, and unlike every saved route's, because a
    /// duplicate would just re-collide.
    public func isValidNewRouteName(_ name: String) -> Bool {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        return !trimmed.isEmpty && plannedRoute(named: trimmed) == nil
    }

    /// Accept the prompt's name and open the landing as a plain new import, not a replace.
    public func confirmNewName(_ name: String) {
        guard let pending = addAsNewPrompt, isValidNewRouteName(name) else { return }
        pendingImport = pending.renamed(to: name.trimmingCharacters(in: .whitespacesAndNewlines))
        addAsNewPrompt = nil
    }

    public func cancelAddAsNew() {
        addAsNewPrompt = nil
    }

    private static func read(_ url: URL) async -> Data? {
        await Task.detached(priority: .userInitiated) { () -> Data? in
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            return try? Data(contentsOf: url)
        }.value
    }

    // MARK: Closing the cover

    /// Close the cover: a completed hand-off, or a plain cancel.
    public func closeImport() {
        pendingImport = nil
    }

    // MARK: Collision lookup

    /// The saved planned route whose name matches, case-insensitively. Reads the library store
    /// directly, because a share can arrive while the launch gate is still connecting, before the
    /// main screen and its in-memory mirror ever started. The store is always current.
    private func plannedRoute(named name: String) -> PlannedRouteRecord? {
        library.plannedRoutes().plannedRoute(named: name)
    }
}

/// The name-collision rule, shared by the import flow and `MainScreenModel.plannedRoute(named:)`:
/// a saved planned route whose name matches the trimmed, lowercased target. Saved names are
/// already trimmed, because they come from the same prompt and decoder paths.
extension Sequence where Element == PlannedRouteRecord {
    public func plannedRoute(named name: String) -> PlannedRouteRecord? {
        let target = name.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return first { $0.summary.name.lowercased() == target }
    }
}

/// A decoded route file on its way to the library: the state behind the cover, from arrival until
/// an action lands it, or Cancel drops it.
public struct PendingImport: Identifiable, Sendable {
    public let id = UUID()
    public var route: ImportedRoute
    public let fileName: String
    /// The original bytes, kept for the library record.
    public let fileData: Data
    public let source: ImportSource
    /// Bond state at arrival, which picks the framing.
    public let noDevicePaired: Bool
    /// The rider's last-used type at arrival.
    public let bikeType: BikeType
    /// The existing route this import replaces, or nil for a fresh import. Its id and device
    /// object id carry through.
    public var replacing: PlannedRouteRecord? = nil

    public init(
        route: ImportedRoute,
        fileName: String,
        fileData: Data,
        source: ImportSource = .file,
        noDevicePaired: Bool,
        bikeType: BikeType = .road,
        replacing: PlannedRouteRecord? = nil
    ) {
        self.route = route
        self.fileName = fileName
        self.fileData = fileData
        self.source = source
        self.noDevicePaired = noDevicePaired
        self.bikeType = bikeType
        self.replacing = replacing
    }

    /// A copy pinned to replace `record`, chosen from the collision dialog.
    public func replacing(_ record: PlannedRouteRecord) -> PendingImport {
        var copy = self
        copy.replacing = record
        return copy
    }

    /// A copy under a fresh name: a plain new import, not a replace.
    public func renamed(to newName: String) -> PendingImport {
        var copy = self
        copy.route.name = newName
        copy.replacing = nil
        return copy
    }

    /// The library record an action lands: the landing's summary over the canonical parsed route
    /// and the original file. A replace keeps the route it is replacing on the device, with its
    /// device link under its old fingerprint, so the badge honestly reads out of date until the
    /// next push. An upload committed during the import flow records its link afterwards through
    /// `markRouteUploaded`, the one place that can scope a link to the connected device; nothing
    /// here mints links.
    public func record(for detail: RouteDetail) -> PlannedRouteRecord {
        PlannedRouteRecord(
            summary: detail.summary,
            route: route,
            bikeType: bikeType,
            sourceFileName: fileName,
            sourceFileData: fileData,
            deviceLink: replacing?.deviceLink,
            uploadedCRC32: replacing?.uploadedCRC32
        )
    }
}

/// Several route files that arrived together, in arrival order.
public struct PendingJoin: Identifiable, Sendable {
    public let id = UUID()
    public let files: [PendingImport]

    public init(files: [PendingImport]) {
        self.files = files
    }
}

/// A just-imported route whose name matches one already in the library: the data behind the
/// update-or-add confirmation dialog.
public struct ImportCollision: Identifiable, Sendable {
    public let id = UUID()
    public let pending: PendingImport
    public let existing: PlannedRouteRecord

    public init(pending: PendingImport, existing: PlannedRouteRecord) {
        self.pending = pending
        self.existing = existing
    }
}
