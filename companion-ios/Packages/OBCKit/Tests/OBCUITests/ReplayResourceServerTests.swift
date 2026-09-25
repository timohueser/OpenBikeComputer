import Foundation
import Testing
@testable import OBCUI

@Suite("Replay bundled resources")
struct ReplayResourceServerTests {
    @Test func pathsCannotEscapeResourceRoot() {
        let root = URL(fileURLWithPath: "/tmp/replay-bundle")
        #expect(ReplayResourceServer.resource(path: "/", root: root)?.lastPathComponent == "index.html")
        #expect(ReplayResourceServer.resource(path: "/cesium/Workers/worker.js", root: root)?.path == root.resolvingSymlinksInPath().appendingPathComponent("cesium/Workers/worker.js").path)
        for path in ["/../secret", "/%2e%2e/secret", "/%2f../secret", "/bad%00file", "/bad\\file", "relative.js"] {
            #expect(ReplayResourceServer.resource(path: path, root: root) == nil)
        }
    }

    @Test func runtimeAndNoticesAreBundled() throws {
        let root = try #require(ReplayResourceServer.bundledRoot)
        for path in ["index.html", "cesium/Cesium.js", "cesium/LICENSE.md"] {
            #expect(FileManager.default.fileExists(atPath: root.appendingPathComponent(path).path))
        }
    }
}
