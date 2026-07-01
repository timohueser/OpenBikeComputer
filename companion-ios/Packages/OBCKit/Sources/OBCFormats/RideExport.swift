import Foundation
import OBCDomain

/// One interchange file format a tracked ride exports to. An encoder is a pure
/// `Ride → Data` function; B7's share/services flows encode through the registry
/// below, never a hardcoded format.
public protocol RideFileEncoder: Sendable {
    /// Lowercase extension of the produced file (e.g. `"gpx"`, `"fit"`).
    var fileExtension: String { get }
    func encode(_ ride: Ride) throws -> Data
}

/// An encoded ride ready for a share sheet / Files / a connected service.
public struct ExportedRideFile: Equatable, Sendable {
    public let fileExtension: String
    public let data: Data

    public init(fileExtension: String, data: Data) {
        self.fileExtension = fileExtension
        self.data = data
    }
}

/// The export edge. **Switching the app's tracked-file format (GPX → FIT) is:
/// add the new `RideFileEncoder` conformer and change `defaultFileExtension` at
/// the composition root.** Every consumer exports through here from the canonical
/// `Ride`, so storage, sync, and screens are untouched by a format change.
public struct RideExporter: Sendable {
    private let encoders: [any RideFileEncoder]
    /// The format used when the caller doesn't ask for one.
    public let defaultFileExtension: String

    public init(encoders: [any RideFileEncoder], defaultFileExtension: String) {
        self.encoders = encoders
        self.defaultFileExtension = defaultFileExtension.lowercased()
    }

    /// Every format the app can export — a future per-service or per-share picker.
    public var supportedFileExtensions: Set<String> {
        Set(encoders.map(\.fileExtension))
    }

    /// Encode `ride` as `fileExtension` (default format when `nil`). Throws
    /// `FormatError.unsupportedFileType` for a format with no registered encoder.
    public func export(_ ride: Ride, as fileExtension: String? = nil) throws -> ExportedRideFile {
        let ext = (fileExtension ?? defaultFileExtension).lowercased()
        guard let encoder = encoders.first(where: { $0.fileExtension == ext }) else {
            throw FormatError.unsupportedFileType(fileExtension: ext)
        }
        return ExportedRideFile(fileExtension: ext, data: try encoder.encode(ride))
    }
}
