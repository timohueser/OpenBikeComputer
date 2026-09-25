import Foundation
import CoreFoundation
import Observation

@MainActor @Observable
final class ReplayPlayerModel {
    enum Phase: Equatable { case loading, ready, failed(String) }
    enum Camera: String { case auto, adjusted, overview }

    let content: ReplayContent
    var phase: Phase = .loading
    private(set) var distance = 0.0
    private(set) var playing = false
    private(set) var camera = Camera.auto
    private(set) var speed = 1.0
    private(set) var photo: ReplayPhoto?
    private(set) var generation = 0
    var reducedMotion = false
    @ObservationIgnored var send: (([String: Any]) -> Void)?
    @ObservationIgnored private var hold: Task<Void, Never>?
    @ObservationIgnored private var active = true
    @ObservationIgnored private var cameraSnapshot: [String: Any]?
    @ObservationIgnored private var shownPhotos: Set<String> = []

    init(content: ReplayContent) { self.content = content }

    var day: String? { content.days.last { $0.distance <= distance }?.name }
    var elevation: Double? {
        let points = content.points
        var lower = 0
        var upper = points.count
        while lower < upper {
            let middle = (lower + upper) / 2
            if points[middle].distance <= distance { lower = middle + 1 } else { upper = middle }
        }
        guard lower > 0 else { return nil }
        let index = lower - 1
        let start = points[index]
        guard let elevation = start.elevation else { return nil }
        guard index + 1 < points.count else { return elevation }
        let end = points[index + 1]
        guard !end.segmentStart, let next = end.elevation, end.distance > start.distance else { return elevation }
        let fraction = (distance - start.distance) / (end.distance - start.distance)
        return elevation + (next - elevation) * fraction
    }

    func receive(_ body: Any) {
        guard active, let event = body as? [String: Any], let type = event["type"] as? String else { return }
        switch type {
        case "booted":
            guard phase == .loading else { return }
            let points: [[String: Any]] = content.points.map {
                ["latitude": $0.latitude, "longitude": $0.longitude,
                 "elevation": $0.elevation.map { $0 as Any } ?? NSNull(),
                 "distance": $0.distance, "segmentStart": $0.segmentStart]
            }
            send?(["type": "load", "track": ["points": points, "durationSeconds": content.durationSeconds],
                   "reducedMotion": reducedMotion])
        case "ready":
            guard phase == .loading else { return }
            phase = .ready
            send?(["type": "seek", "distance": distance])
            send?(["type": "speed", "value": speed])
            if let cameraSnapshot {
                send?(["type": "restoreCamera", "snapshot": cameraSnapshot])
            } else {
                send?(["type": "camera", "mode": camera == .overview ? "overview" : "auto"])
            }
        case "progress":
            guard phase == .ready,
                  let value = Self.number(event["distance"]), value >= 0, value <= content.totalDistance + 0.01,
                  let mode = event["mode"] as? String, let mode = Camera(rawValue: mode),
                  let isPlaying = Self.boolean(event["playing"]) else { return }
            let previous = distance
            // A queued pre-seek tick cannot move a paused playhead back to its old position.
            guard playing || abs(value - distance) < 0.01 else { return }
            camera = mode
            if let snapshot = Self.cameraSnapshot(event["cameraState"]), snapshot["mode"] as? String == mode.rawValue {
                cameraSnapshot = snapshot
            }
            distance = min(value, content.totalDistance)
            if playing, value >= previous,
               let next = content.photos.first(where: {
                   $0.thumbnailData != nil && $0.distance >= previous && $0.distance <= value && !shownPhotos.contains($0.id)
               }) {
                showPhoto(next, automatic: true)
            } else if !isPlaying, distance >= content.totalDistance {
                playing = false
            }
        case "error": fail("The map cannot load. Check your connection and try again.")
        default: break
        }
    }

    static func cameraSnapshot(_ value: Any?) -> [String: Any]? {
        guard let value = value as? [String: Any],
              let mode = value["mode"] as? String, Camera(rawValue: mode) != nil,
              let follow = value["followMode"] as? String, ["auto", "adjusted"].contains(follow),
              mode == "overview" || mode == follow,
              let offsets = value["offsets"] as? [String: Any],
              let heading = number(offsets["heading"]), abs(heading) <= .pi,
              let pitch = number(offsets["pitch"]), abs(pitch) <= .pi / 3,
              let range = number(offsets["range"]), (0.17...5).contains(range) else { return nil }
        return ["mode": mode, "followMode": follow,
                "offsets": ["heading": heading, "pitch": pitch, "range": range]]
    }

    static func number(_ value: Any?) -> Double? {
        guard let number = value as? NSNumber,
              CFGetTypeID(number) != CFBooleanGetTypeID(), number.doubleValue.isFinite else { return nil }
        return number.doubleValue
    }

    static func boolean(_ value: Any?) -> Bool? {
        guard let number = value as? NSNumber, CFGetTypeID(number) == CFBooleanGetTypeID() else { return nil }
        return number.boolValue
    }

    func togglePlayback() {
        guard phase == .ready else { return }
        if photo != nil { continuePhoto(); return }
        if playing { pause(); return }
        if distance >= content.totalDistance { seek(0) }
        playing = true
        send?(["type": "play"])
    }

    func pause() {
        cancelPhoto()
        playing = false
        send?(["type": "pause"])
    }

    func seek(_ value: Double) {
        guard phase == .ready, value.isFinite else { return }
        pause()
        distance = min(max(value, 0), content.totalDistance)
        shownPhotos = Set(content.photos.filter { $0.distance <= distance }.map(\.id))
        send?(["type": "seek", "distance": distance])
    }

    func setSpeed(_ value: Double) {
        guard [0.5, 1, 2].contains(value) else { return }
        speed = value
        send?(["type": "speed", "value": value])
    }

    func setCamera(_ mode: String) {
        guard phase == .ready, ["auto", "overview", "follow"].contains(mode) else { return }
        send?(["type": "camera", "mode": mode])
    }

    func showPhoto(_ value: ReplayPhoto, automatic: Bool = false) {
        guard phase == .ready else { return }
        pause()
        // Resume at this moment so a single tick cannot skip nearby photos.
        distance = value.distance
        send?(["type": "seek", "distance": distance])
        shownPhotos.insert(value.id)
        photo = value
        if automatic {
            hold = Task { [weak self] in
                do { try await Task.sleep(for: .seconds(3)) } catch { return }
                guard let self, self.active else { return }
                self.continuePhoto()
            }
        }
    }

    func continuePhoto() {
        cancelPhoto()
        guard active, phase == .ready, distance < content.totalDistance else { return }
        playing = true
        send?(["type": "play"])
    }

    private func cancelPhoto() { hold?.cancel(); hold = nil; photo = nil }

    func fail(_ message: String) {
        if case .failed = phase { return }
        pause()
        phase = .failed(message)
    }

    func suspend() {
        pause()
        active = false
        send?(["type": "destroy"])
        send = nil
    }

    func retry() {
        pause()
        active = true
        phase = .loading
        generation += 1
    }
}
