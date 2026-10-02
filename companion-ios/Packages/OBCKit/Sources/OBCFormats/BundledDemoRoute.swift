import Foundation

public enum BundledDemoRoute {
    public static let fileName = "grimsel-pass.gpx"

    public static func data() throws -> Data {
        guard let url = Bundle.module.url(forResource: "grimsel-pass", withExtension: "gpx") else {
            throw CocoaError(.fileNoSuchFile)
        }
        return try Data(contentsOf: url)
    }
}
