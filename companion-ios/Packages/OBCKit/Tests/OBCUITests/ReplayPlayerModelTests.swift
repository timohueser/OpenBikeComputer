import Foundation
import Testing
@testable import OBCUI

@MainActor @Suite("Replay player state")
struct ReplayPlayerModelTests {
    private func content(photos: [ReplayPhoto] = []) -> ReplayContent {
        ReplayContent(title: "Test ride", points: [
            ReplayPoint(latitude: 48, longitude: 8, elevation: 100, distance: 0, segmentStart: true),
            ReplayPoint(latitude: 48.01, longitude: 8, elevation: 200, distance: 100, segmentStart: false),
            ReplayPoint(latitude: 49, longitude: 9, elevation: nil, distance: 100, segmentStart: true),
            ReplayPoint(latitude: 49.01, longitude: 9, elevation: nil, distance: 200, segmentStart: false),
        ], totalDistance: 200, durationSeconds: 60,
        days: [ReplayDay(name: "Day 1", distance: 0), ReplayDay(name: "Day 2", distance: 100)], photos: photos)
    }

    private func progress(_ model: ReplayPlayerModel, distance: Double, playing: Bool = true, mode: String = "auto") {
        model.receive(["type": "progress", "distance": distance, "playing": playing, "mode": mode])
    }

    @Test func bridgeRejectsInvalidEventsAndStartsPaused() throws {
        let model = ReplayPlayerModel(content: content())
        var commands: [[String: Any]] = []
        model.send = { commands.append($0) }
        model.receive(["type": "booted"])
        let load = try #require(commands.first)
        #expect(JSONSerialization.isValidJSONObject(load))
        #expect(load["type"] as? String == "load")
        let track = try #require(load["track"] as? [String: Any])
        let points = try #require(track["points"] as? [[String: Any]])
        #expect(points[2]["elevation"] is NSNull)
        model.receive(["type": "ready"])
        #expect(model.phase == .ready)
        #expect(!model.playing)
        model.togglePlayback()
        for distance in [Double.nan, .infinity, -1, 201] { progress(model, distance: distance) }
        model.receive(["type": "progress", "distance": true, "playing": true, "mode": "auto"])
        progress(model, distance: 10, mode: "unknown")
        #expect(model.distance == 0)
        progress(model, distance: 20)
        #expect(model.distance == 20)
    }

    @Test func scrubbingPausesAndIgnoresQueuedTicks() {
        let model = ReplayPlayerModel(content: content())
        model.receive(["type": "ready"])
        model.togglePlayback()
        progress(model, distance: 10)
        model.seek(75)
        progress(model, distance: 11)
        #expect(!model.playing)
        #expect(model.distance == 75)
        #expect(model.elevation == 175)
        #expect(model.day == "Day 1")
        model.seek(100)
        #expect(model.elevation == nil)
        #expect(model.day == "Day 2")
        model.seek(.nan)
        #expect(model.distance == 100)
        model.seek(500)
        #expect(model.distance == 200)
    }

    @Test func photoCrossingsSkipMissingImagesAndResetOnlyOnNewTraversal() {
        let missing = ReplayPhoto(id: "missing", distance: 10, thumbnailData: nil)
        let available = ReplayPhoto(id: "available", distance: 20, thumbnailData: Data([1]))
        let model = ReplayPlayerModel(content: content(photos: [missing, available]))
        model.receive(["type": "ready"])
        model.togglePlayback()
        progress(model, distance: 11)
        #expect(model.photo == nil)
        #expect(model.playing)
        progress(model, distance: 21)
        #expect(model.photo?.id == "available")
        #expect(!model.playing)
        model.continuePhoto()
        progress(model, distance: 22)
        #expect(model.photo == nil)
        model.seek(0)
        progress(model, distance: 21)
        #expect(model.photo == nil)
        model.togglePlayback()
        progress(model, distance: 21)
        #expect(model.photo?.id == "available")
        model.suspend()
        #expect(model.photo == nil)
        #expect(!model.playing)
    }

    @Test func suspensionAndRetryPreservePositionButNeverResumePlayback() {
        let model = ReplayPlayerModel(content: content())
        var commands: [[String: Any]] = []
        model.send = { commands.append($0) }
        model.receive(["type": "ready"])
        model.togglePlayback()
        progress(model, distance: 70, mode: "overview")
        model.suspend()
        progress(model, distance: 80)
        #expect(model.distance == 70)
        #expect(!model.playing)
        #expect(commands.last?["type"] as? String == "destroy")
        model.retry()
        model.send = { commands.append($0) }
        model.receive(["type": "ready"])
        #expect(model.generation == 1)
        #expect(model.distance == 70)
        #expect(model.camera == .overview)
        #expect(!model.playing)
        #expect(commands.last?["mode"] as? String == "overview")
        model.fail("Unavailable")
        #expect(model.phase == .failed("Unavailable"))
        model.togglePlayback()
        #expect(!model.playing)
    }

    @Test func oneTickCannotSkipNearbyPhotoMoments() {
        let photos = [20.0, 21.0, 21.0].enumerated().map { index, distance in
            ReplayPhoto(id: "photo-\(index)", distance: distance, thumbnailData: Data([1]))
        }
        let model = ReplayPlayerModel(content: content(photos: photos))
        model.receive(["type": "ready"])
        model.togglePlayback()
        for photo in photos {
            progress(model, distance: 30)
            #expect(model.photo?.id == photo.id)
            #expect(model.distance == photo.distance)
            model.continuePhoto()
        }
        progress(model, distance: 30)
        #expect(model.photo == nil)
        #expect(model.playing)
        model.suspend()
    }

    @Test func rendererReplacementRestoresValidatedCameraOffsets() throws {
        let model = ReplayPlayerModel(content: content())
        let snapshot: [String: Any] = ["mode": "overview", "followMode": "adjusted",
                                      "offsets": ["heading": 0.7, "pitch": -0.2, "range": 1.5]]
        model.receive(["type": "ready"])
        model.togglePlayback()
        model.receive(["type": "progress", "distance": 70.0, "playing": true,
                       "mode": "overview", "cameraState": snapshot])
        model.suspend()
        model.retry()
        var commands: [[String: Any]] = []
        model.send = { commands.append($0) }
        model.receive(["type": "ready"])
        let restore = try #require(commands.last)
        #expect(restore["type"] as? String == "restoreCamera")
        let restored = try #require(restore["snapshot"] as? [String: Any])
        #expect(restored["mode"] as? String == "overview")
        #expect(restored["followMode"] as? String == "adjusted")
        #expect((restored["offsets"] as? [String: Double])?["heading"] == 0.7)
        #expect(!model.playing)
        #expect(ReplayPlayerModel.cameraSnapshot(["mode": "overview", "followMode": "adjusted",
                                                "offsets": ["heading": true, "pitch": 0, "range": 1]]) == nil)
        #expect(ReplayPlayerModel.cameraSnapshot(["mode": "overview", "followMode": "adjusted",
                                                "offsets": ["heading": 0, "pitch": 0, "range": 99]]) == nil)
    }

}
