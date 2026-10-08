import Foundation
import Observation
import OBCDomain
import OBCPlanner
import OBCTransport

/// What an import does with a file's line.
public enum ImportChoice: Sendable {
    /// One drawn leg with the file's line.
    case keepLine
    /// Shaping points on roads that reproduce the line, routed.
    case planOnRoads
}

/// The import flow's state machine, extracted from the composition root so it runs under `swift test`:
/// files arrive and decode, one choice for all of them keeps their lines or plans them on roads, and then
/// one file lands as a route, through the update-or-add dialog when its name is taken, and several files
/// open the "Make a trip" sheet.
///
/// Formats stay at the edges: OBCUI must not import `OBCFormats`, so the decode is injected as a
/// closure over the composition root's importer.
@MainActor @Observable
public final class ImportFlowModel {
    // MARK: Observable state, which the view binds

    /// Decoded files that wait for the keep-or-plan choice; nil closes the sheet.
    public var pendingChoice: [PendingImport]?
    /// While the files are planned on roads, one request at a time: the file in work, from 1, and the count.
    public private(set) var planning: (file: Int, of: Int)?
    /// One plain line about the import, for an alert.
    public var notice: String?
    /// One file ready to save. The composition root saves it, opens its detail and clears this.
    public var landed: PendingImport?
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
    /// The choice made on the sheet; it runs once the sheet has gone.
    @ObservationIgnored private var chosen: (choice: ImportChoice, files: [PendingImport])?
    @ObservationIgnored private var planningTask: Task<Void, Never>?

    // MARK: Injected seams

    private let decode: (Data, String) throws -> ImportedRoute
    /// The phone-side library, read directly for the collision check; see `plannedRoute(named:)`.
    private let library: any LibraryStore
    private let lastBikeType: LastBikeTypeStore

    public init(
        decode: @escaping (Data, String) throws -> ImportedRoute,
        library: any LibraryStore,
        lastBikeType: LastBikeTypeStore = LastBikeTypeStore(),
        batchWindow: Duration = .milliseconds(400)
    ) {
        self.batchWindow = batchWindow
        self.decode = decode
        self.library = library
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

    /// Picked or shared files. The security-scoped access and the read happen off the main actor,
    /// because a share-sheet file that lives in iCloud Drive and is not downloaded yet blocks inside
    /// `Data(contentsOf:)` until the download lands. An unreadable file fails as an undecodable one does.
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

    /// Route files in hand open the choice, or the alert when one does not decode.
    public func open(files: [(data: Data, fileName: String)]) {
        guard !refusedWhilePlanning() else { return }
        var pending: [PendingImport] = []
        for file in files {
            guard let route = try? decode(file.data, file.fileName) else {
                importFailed = true
                return
            }
            pending.append(PendingImport(route: route, fileName: file.fileName, fileData: file.data, bikeType: lastBikeType.value))
        }
        if !pending.isEmpty { pendingChoice = pending }
    }

    /// A route already in hand, such as a ride saved as a route: its line is kept, and the
    /// name-collision rule of a file applies. A nil `bikeType` takes the last one picked.
    public func open(route: ImportedRoute, fileName: String, fileData: Data, bikeType: BikeType? = nil) {
        guard !refusedWhilePlanning() else { return }
        place([PendingImport(route: route, fileName: fileName, fileData: fileData, bikeType: bikeType ?? lastBikeType.value)])
    }

    /// One import plans at a time, so the shape requests never run in parallel.
    private func refusedWhilePlanning() -> Bool {
        guard planning != nil else { return false }
        notice = "An import is still planning on roads. Try again when it is done."
        return true
    }

    // MARK: The keep-or-plan choice

    /// The choice for every pending file, from the sheet. ``proceed(using:)`` runs it once the sheet
    /// has gone, so the next sheet does not open over it.
    public func choose(_ choice: ImportChoice) {
        guard let files = pendingChoice else { return }
        chosen = (choice, files)
        pendingChoice = nil
    }

    public func cancelChoice() {
        pendingChoice = nil
    }

    /// Runs the choice: a kept line lands at once; planning on roads returns its task.
    @discardableResult
    public func proceed(using source: any PlannerDataSource) -> Task<Void, Never>? {
        guard let (choice, files) = chosen.map({ ($0.choice, $0.files) }) else { return nil }
        chosen = nil
        guard choice == .planOnRoads else {
            place(files)
            return nil
        }
        planning = (1, files.count)
        planningTask = Task { [weak self] in
            var planned: [PendingImport] = []
            for (index, file) in files.enumerated() {
                // After Cancel, the other files keep their lines without a request.
                guard !Task.isCancelled else { planned.append(file); continue }
                self?.planning = (index + 1, files.count)
                do { planned.append(try await Self.planOnRoads(file, using: source)) }
                catch {
                    var kept = file
                    if !Task.isCancelled { kept.keptReason = Self.reason(error) }
                    planned.append(kept)
                }
            }
            guard let self else { return }
            planning = nil
            planningTask = nil
            place(planned)
        }
        return planningTask
    }

    /// Stops planning: the files planned so far keep their plans, the others their lines, and they save as usual.
    public func cancelPlanning() {
        planningTask?.cancel()
    }

    /// The shape of the file's line with the Balanced profile of its bike type, then one route through
    /// the shape points. The file's waypoints stay as markers, placed on the routed line.
    private static func planOnRoads(_ file: PendingImport, using source: any PlannerDataSource) async throws -> PendingImport {
        let activity = RouteActivity(file.bikeType)
        let shape = try await source.shape(line: file.route.points.map(\.coordinate),
                                           profile: RoutePreference.balanced.profile(for: activity))
        let path = try await source.route(points: shape.points, turnarounds: shape.turnarounds, activity: activity,
                                          preference: .balanced, release: source.release())
        guard path.pointIndices.count == shape.points.count, MeasuredLine(routePoints: path.points).length > 0,
              let plan = PlannerPlan.shaped(shape.points, turnarounds: shape.turnarounds, waypoints: file.route.waypoints)
        else { throw PlannerFailure.invalidData }
        var planned = file
        planned.route.points = path.points
        planned.route.waypoints = Waypoint.placed(file.route.waypoints, along: path.points)
        planned.plan = file.withBike(plan)
        return planned
    }

    /// Why a file keeps its line, in a few plain words.
    static func reason(_ error: Error) -> String {
        switch error as? PlannerFailure {
        case .lineTooLong: "the line is too long to plan"
        case .lineNotReproducible, .noRoad: "no route on roads follows it"
        case .busy: "the route service is busy"
        case .outsideRegion: "it is outside the map region"
        case .unavailable: "the route service is unavailable"
        case .offlineUnavailable: "no connection and no offline map"
        default: "it could not be planned"
        }
    }

    /// The one line for the files of an import that kept their line although they were to be planned on
    /// roads; nil when none did.
    public static func notice(for files: [PendingImport]) -> String? {
        let kept = files.filter { $0.keptReason != nil }
        guard !kept.isEmpty else { return nil }
        if files.count == 1, let reason = kept[0].keptReason { return "Kept the file's line: \(reason)." }
        return kept.map { "Kept the line of \u{201C}\($0.fileName)\u{201D}: \($0.keptReason ?? "")." }.joined(separator: " ")
    }

    /// One file lands, through the update-or-add dialog when a saved route has its name; several open
    /// the "Make a trip" sheet.
    private func place(_ files: [PendingImport]) {
        guard files.count == 1, let file = files.first else {
            if !files.isEmpty { pendingJoin = PendingJoin(files: files) }
            return
        }
        if let existing = plannedRoute(named: file.route.name ?? file.fileName) {
            collision = ImportCollision(pending: file, existing: existing)
        } else {
            landed = file
        }
    }

    public func closeJoin() {
        pendingJoin = nil
    }

    // MARK: The collision dialog

    /// "Update the existing route": the import replaces the saved record, whose id and device link
    /// carry through.
    public func chooseReplace() {
        guard let collision else { return }
        landed = collision.pending.replacing(collision.existing)
        self.collision = nil
    }

    /// "Add as a new route": two routes under one name would be indistinguishable, and the next
    /// import's collision check keys on the name, so a distinct name is required first. This
    /// detours through the rename prompt.
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

    /// Accept the prompt's name and land the import as a plain new route, not a replace.
    public func confirmNewName(_ name: String) {
        guard let pending = addAsNewPrompt, isValidNewRouteName(name) else { return }
        landed = pending.renamed(to: name.trimmingCharacters(in: .whitespacesAndNewlines))
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

/// A decoded route file on its way to the library, with the plan it is saved with.
public struct PendingImport: Identifiable, Sendable {
    public let id = UUID()
    public var route: ImportedRoute
    public let fileName: String
    /// The original bytes, kept for the library record.
    public let fileData: Data
    /// The rider's last-used type at arrival.
    public let bikeType: BikeType
    /// The file's line kept as it is, until it is planned on roads.
    public var plan: PlannerPlan?
    /// Why the line was kept although it was to be planned on roads.
    public var keptReason: String?
    /// The existing route this import replaces, or nil for a fresh import. Its id and device
    /// object id carry through.
    public var replacing: PlannedRouteRecord? = nil

    public init(route: ImportedRoute, fileName: String, fileData: Data, bikeType: BikeType = .road) {
        self.route = route
        self.fileName = fileName
        self.fileData = fileData
        self.bikeType = bikeType
        plan = PlannerPlan.keptLine(route.points, waypoints: route.waypoints).map(withBike)
    }

    /// `plan` under this file's bike type.
    func withBike(_ plan: PlannerPlan) -> PlannerPlan {
        var plan = plan
        plan.bike = RouteActivity(bikeType).rawValue
        return plan
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

    /// Save and upload share the import's identity. A replacement keeps the saved route's id.
    public var routeID: RouteID { replacing?.id ?? RouteID("imported-\(id.uuidString.lowercased())") }

    /// A replacement keeps the device link and its old fingerprint until the next upload.
    public func record() -> PlannedRouteRecord {
        PlannedRouteRecord(
            route: route,
            id: routeID,
            bikeType: bikeType,
            sourceFileName: fileName,
            sourceFileData: fileData,
            deviceLink: replacing?.deviceLink,
            uploadedCRC32: replacing?.uploadedCRC32,
            plan: plan
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
