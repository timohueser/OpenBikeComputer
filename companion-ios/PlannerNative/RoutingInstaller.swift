import Foundation
import OBCPlanner
import os

/// The routing installer of the planner-service library. Each call runs on one serial queue, so no
/// blocking call runs on the cooperative pool.
struct RoutingInstaller: OfflineRouting {
    private let queue = DispatchQueue(label: "com.openbikecomputer.routing-installer", qos: .utility)

    func plan(package: URL, directory: URL, target: URL, areas: [[Double]]) async throws -> OfflineRoutingPlan {
        let request = try Self.request(package: package, directory: directory, target: target, areas: areas, objects: nil)
        let answer = try await run(InstallProgress(nil)) {
            var error: UnsafeMutablePointer<CChar>?
            guard let answer = request.withCString({ planner_routing_plan($0, &error) }) else { throw Self.failure(error) }
            defer { planner_response_free(answer) }
            return Data(bytes: answer, count: strlen(answer))
        }
        return try JSONDecoder().decode(OfflineRoutingPlan.self, from: answer)
    }

    func install(package: URL, directory: URL, target: URL, areas: [[Double]], objects: URL,
                 progress: @escaping @Sendable (Int64, Int64) -> Void) async throws {
        let request = try Self.request(package: package, directory: directory, target: target, areas: areas,
                                       objects: objects)
        let box = InstallProgress(progress)
        try await run(box) {
            let context = Unmanaged.passRetained(box).toOpaque()
            defer { Unmanaged<InstallProgress>.fromOpaque(context).release() }
            var error: UnsafeMutablePointer<CChar>?
            // The callback runs on a Rust worker thread, which has no current task: it reads the flag only.
            let done = request.withCString {
                planner_routing_install($0, { context, done, total in
                    guard let context else { return false }
                    let box = Unmanaged<InstallProgress>.fromOpaque(context).takeUnretainedValue()
                    box.report?(Int64(clamping: done), Int64(clamping: total))
                    return !box.stopped.withLock { $0 }
                }, context, &error)
            }
            if !done { throw Self.failure(error) }
        }
    }

    /// Runs `work` on the queue. A cancellation sets the stop flag of `progress`.
    private func run<T: Sendable>(_ progress: InstallProgress, _ work: @escaping @Sendable () throws -> T) async throws -> T {
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                queue.async { continuation.resume(with: Result { try work() }) }
            }
        } onCancel: {
            progress.stopped.withLock { $0 = true }
        }
    }

    private static func request(package: URL, directory: URL, target: URL, areas: [[Double]],
                                objects: URL?) throws -> String {
        var request: [String: Any] = ["package": package.path(percentEncoded: false),
                                      "directory": directory.path(percentEncoded: false),
                                      "target": target.path(percentEncoded: false), "areas": areas]
        if let objects { request["objects"] = objects.path(percentEncoded: false) }
        return String(decoding: try JSONSerialization.data(withJSONObject: request), as: UTF8.self)
    }

    /// The error of a failed call: `{"status", "body": {"code", "message"}}`, which this frees.
    private static func failure(_ error: UnsafeMutablePointer<CChar>?) -> Error {
        struct Failure: Decodable {
            struct Body: Decodable { let code: String; let message: String }
            let status: Int
            let body: Body
        }
        guard let error else {
            return NSError(domain: "PlannerRouting", code: 1,
                           userInfo: [NSLocalizedDescriptionKey: "The routing installer returned no answer"])
        }
        defer { planner_response_free(error) }
        guard let failure = try? JSONDecoder().decode(Failure.self, from: Data(bytes: error, count: strlen(error))) else {
            return NSError(domain: "PlannerRouting", code: 1,
                           userInfo: [NSLocalizedDescriptionKey: "The routing installer failed"])
        }
        if failure.body.code == "cancelled" { return CancellationError() }
        // A missing or damaged object, or a damaged directory.
        if failure.body.code == "invalid_data" { return PlannerFailure.invalidData }
        return NSError(domain: "PlannerRouting", code: failure.status,
                       userInfo: [NSLocalizedDescriptionKey: failure.body.message])
    }
}

/// The context of the progress callback: the stop flag that a cancellation sets, and the progress of the
/// store.
private final class InstallProgress: Sendable {
    let stopped = OSAllocatedUnfairLock(initialState: false)
    let report: (@Sendable (Int64, Int64) -> Void)?
    init(_ report: (@Sendable (Int64, Int64) -> Void)?) { self.report = report }
}
