import Foundation
import Observation
import OBCDomain
import OBCTransport

/// The import flow's state machine (E1/H4/H5), extracted from the composition
/// root so the replace-vs-new and rename rules run under `swift test`: a file
/// arrives (share sheet, Files pick, or `-OBCImportSample` at launch), decodes
/// into the pending import behind the E1 cover, and a name collision detours
/// through the update-or-add dialog before the cover opens.
///
/// **Formats stay at the edges:** OBCUI must not import `OBCFormats`, so the
/// *decode* is injected — the composition root passes a closure over its
/// `RouteImporter` (data + file name → `ImportedRoute`); only the *flow* lives
/// here. The bond check is likewise a narrow `isBonded` closure, not the whole
/// `BondStore` — the flow only needs the E1-vs-H4 framing bit.
///
/// The view binds `pendingImport` (full-screen cover), `collision`
/// (confirmation dialog), `addAsNewPrompt` + `newRouteName` (the rename
/// prompt), and `importFailed` (the H5 alert); every transition between them
/// goes through the methods below.
@MainActor @Observable
public final class ImportFlowModel {
    // MARK: Observable state (the view binds these)

    /// The decoded route behind the E1 cover — `nil` closes it.
    public var pendingImport: PendingImport?
    /// A just-imported route whose name matches a saved one — drives the
    /// update-or-add confirmation dialog.
    public var collision: ImportCollision?
    /// "Add as a new route" chosen from the collision dialog — holds the import
    /// while the distinct-name prompt is up.
    public var addAsNewPrompt: PendingImport?
    /// The rename prompt's text field.
    public var newRouteName = ""
    /// The H5 "couldn't read that file" alert.
    public var importFailed = false

    // MARK: Injected seams

    private let decode: (Data, String) throws -> ImportedRoute
    /// The phone-side library (B1S) — read **directly** for the collision
    /// check; see `plannedRoute(named:)`.
    private let library: any LibraryStore
    /// Bond state at arrival picks the E1 vs H4 framing — a narrow closure,
    /// not the whole `BondStore` (the launch flow owns the record itself).
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

    // MARK: Opening a file (→ E1)

    /// A picked/shared file URL: read it, then run the `open(data:fileName:)`
    /// flow. The security-scoped access + read happen **off the main actor** —
    /// a share-sheet file that lives in iCloud Drive and isn't downloaded yet
    /// blocks inside `Data(contentsOf:)` until the download lands, which must
    /// not freeze the UI. An unreadable file is the same H5 failure as an
    /// undecodable one.
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

    /// Route-file bytes in hand (a finished read, or `-OBCImportSample` at
    /// launch): decode, then either open E1 straight away or — when a saved
    /// route already carries this name — offer update-in-place vs new.
    public func open(data: Data, fileName: String) {
        do {
            let route = try decode(data, fileName)
            let pending = PendingImport(
                route: route,
                fileName: fileName,
                fileData: data,
                noDevicePaired: !isBonded()
            )
            // A route by this name already saved → offer update-in-place vs new.
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

    /// "Update the existing route": open E1 pinned to replace the saved record
    /// — its id + device link carry through to the landing.
    public func chooseReplace() {
        guard let collision else { return }
        pendingImport = collision.pending.replacing(collision.existing)
        self.collision = nil
    }

    /// "Add as a new route": two routes under one name would be
    /// indistinguishable (and the next import's collision check keys on the
    /// name) — a distinct name is required before the landing opens, so this
    /// detours through the rename prompt instead of opening E1.
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

    /// Whether the prompt's current name can be accepted: non-empty and unlike
    /// every saved route's (the collision check keys on names, so a duplicate
    /// would just re-collide).
    public var isNewRouteNameValid: Bool {
        let trimmed = newRouteName.trimmingCharacters(in: .whitespacesAndNewlines)
        return !trimmed.isEmpty && plannedRoute(named: trimmed) == nil
    }

    /// Accept the prompt's name and open E1 as a plain new import (no replace).
    public func confirmNewName() {
        guard let pending = addAsNewPrompt, isNewRouteNameValid else { return }
        pendingImport = pending.renamed(to: newRouteName.trimmingCharacters(in: .whitespacesAndNewlines))
        addAsNewPrompt = nil
    }

    public func cancelAddAsNew() {
        addAsNewPrompt = nil
    }

    // MARK: Closing the cover

    /// Close E1 — a completed save/pair hand-off or a plain cancel.
    public func closeImport() {
        pendingImport = nil
    }

    // MARK: Collision lookup

    /// The saved planned route whose name matches, case-insensitively. Reads
    /// the **library store directly** — a share can arrive while the launch
    /// gate is still connecting, before the main screen (and its in-memory
    /// mirror) ever started; the store is always current, every save writes
    /// through it.
    private func plannedRoute(named name: String) -> PlannedRouteRecord? {
        library.plannedRoutes().plannedRoute(named: name)
    }
}

/// **The** name-collision rule, shared by the import flow and
/// `MainScreenModel.plannedRoute(named:)`: a saved planned route whose name
/// matches the trimmed, lowercased target (saved names are already trimmed —
/// they come from the same prompt/decoder paths).
extension Sequence where Element == PlannedRouteRecord {
    public func plannedRoute(named name: String) -> PlannedRouteRecord? {
        let target = name.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return first { $0.summary.name.lowercased() == target }
    }
}

/// A decoded route file on its way to the library — the state behind the E1
/// cover, from arrival until Save/Upload/Pair lands it (or Cancel drops it).
public struct PendingImport: Identifiable, Sendable {
    public let id = UUID()
    public var route: ImportedRoute
    public let fileName: String
    /// The original bytes, kept for the library record (re-parse/debugging).
    public let fileData: Data
    /// Bond state at arrival — picks the E1 vs H4 framing.
    public let noDevicePaired: Bool
    /// The existing route this import replaces (name-collision → Replace), or
    /// `nil` for a fresh import. Its id + device object id carry through.
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

    /// A copy pinned to replace `record` (chosen from the collision dialog).
    public func replacing(_ record: PlannedRouteRecord) -> PendingImport {
        var copy = self
        copy.replacing = record
        return copy
    }

    /// A copy under a fresh name (the "Add as a new route" prompt) — a plain
    /// new import, not a replace.
    public func renamed(to newName: String) -> PendingImport {
        var copy = self
        copy.route.name = newName
        copy.replacing = nil
        return copy
    }

    /// The library record (B1S) a save/upload/pair action lands: the landing's
    /// summary over the canonical parsed route + the original file. A replace
    /// keeps the route it's replacing on the device — its `deviceLink` under
    /// its old fingerprint, so the badge honestly reads **out of date** until
    /// the next push updates the copy. An upload committed during the import
    /// flow records its link afterwards via `markRouteUploaded` (the one place
    /// that can scope a link to the connected device's identity, #769) —
    /// nothing here mints links.
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

/// A just-imported route whose name matches one already in the library — the
/// data behind the update-or-add confirmation dialog.
public struct ImportCollision: Identifiable, Sendable {
    public let id = UUID()
    public let pending: PendingImport
    public let existing: PlannedRouteRecord

    public init(pending: PendingImport, existing: PlannedRouteRecord) {
        self.pending = pending
        self.existing = existing
    }
}
