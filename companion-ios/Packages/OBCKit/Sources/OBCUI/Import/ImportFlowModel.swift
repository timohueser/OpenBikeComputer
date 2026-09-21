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
/// The view binds `pendingImport`, `collision`, `addAsNewPrompt` with `newRouteName`, and
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
    /// The rename prompt's text field.
    public var newRouteName = ""
    /// The "couldn't read that file" alert.
    public var importFailed = false

    // MARK: Injected seams

    private let decode: (Data, String) throws -> ImportedRoute
    /// The phone-side library, read directly for the collision check; see `plannedRoute(named:)`.
    private let library: any LibraryStore
    /// Bond state at arrival picks the framing. A narrow closure, not the whole `BondStore`,
    /// because the launch flow owns the record itself.
    private let isBonded: () -> Bool

    public init(
        decode: @escaping (Data, String) throws -> ImportedRoute,
        library: any LibraryStore,
        isBonded: @escaping () -> Bool
    ) {
        self.decode = decode
        self.library = library
        self.isBonded = isBonded
    }

    // MARK: Opening a file

    /// A picked or shared file URL: read it, then run the `open(data:fileName:)` flow. The
    /// security-scoped access and the read happen off the main actor, because a share-sheet file
    /// that lives in iCloud Drive and is not downloaded yet blocks inside `Data(contentsOf:)`
    /// until the download lands, and that must not freeze the UI. An unreadable file fails the
    /// same way an undecodable one does.
    public func openFile(at url: URL) async {
        let data = await Task.detached(priority: .userInitiated) { () -> Data? in
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            return try? Data(contentsOf: url)
        }.value
        guard let data else {
            importFailed = true
            return
        }
        open(data: data, fileName: url.lastPathComponent)
    }

    /// Route-file bytes in hand: decode, then either open the landing straight away or, when a
    /// saved route already carries this name, offer update-in-place against new.
    public func open(data: Data, fileName: String) {
        do {
            let route = try decode(data, fileName)
            let pending = PendingImport(
                route: route,
                fileName: fileName,
                fileData: data,
                noDevicePaired: !isBonded()
            )
            // A route by this name is already saved, so offer update-in-place against new.
            if let existing = plannedRoute(named: route.name ?? fileName) {
                collision = ImportCollision(pending: pending, existing: existing)
            } else {
                pendingImport = pending
            }
        } catch {
            importFailed = true
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

    /// Whether the prompt's current name can be accepted: non-empty, and unlike every saved
    /// route's, because a duplicate would just re-collide.
    public var isNewRouteNameValid: Bool {
        let trimmed = newRouteName.trimmingCharacters(in: .whitespacesAndNewlines)
        return !trimmed.isEmpty && plannedRoute(named: trimmed) == nil
    }

    /// Accept the prompt's name and open the landing as a plain new import, not a replace.
    public func confirmNewName() {
        guard let pending = addAsNewPrompt, isNewRouteNameValid else { return }
        pendingImport = pending.renamed(to: newRouteName.trimmingCharacters(in: .whitespacesAndNewlines))
        addAsNewPrompt = nil
    }

    public func cancelAddAsNew() {
        addAsNewPrompt = nil
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
    /// Bond state at arrival, which picks the framing.
    public let noDevicePaired: Bool
    /// The existing route this import replaces, or nil for a fresh import. Its id and device
    /// object id carry through.
    public var replacing: PlannedRouteRecord? = nil

    public init(
        route: ImportedRoute,
        fileName: String,
        fileData: Data,
        noDevicePaired: Bool,
        replacing: PlannedRouteRecord? = nil
    ) {
        self.route = route
        self.fileName = fileName
        self.fileData = fileData
        self.noDevicePaired = noDevicePaired
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
            sourceFileName: fileName,
            sourceFileData: fileData,
            deviceLink: replacing?.deviceLink,
            uploadedCRC32: replacing?.uploadedCRC32
        )
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
