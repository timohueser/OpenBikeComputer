import Foundation
import OBCDomain

/// One interchange file format the app can import a planned route from. A decoder is
/// a pure `Data` to `ImportedRoute` function, with no transport and no UI, so each
/// format is a single unit-testable conformer.
public protocol RouteFileDecoder: Sendable {
    /// Lowercase file extensions this decoder claims (e.g. `["gpx"]`).
    var fileExtensions: Set<String> { get }
    func decode(_ data: Data) throws -> ImportedRoute
}

/// The import edge: picks the decoder for a file by extension and runs it. Adding a
/// format means appending one `RouteFileDecoder` at the composition root; nothing
/// downstream changes.
public struct RouteImporter: Sendable {
    private let decoders: [any RouteFileDecoder]

    public init(decoders: [any RouteFileDecoder]) {
        self.decoders = decoders
    }

    /// Every extension the app accepts. Drives the document-picker filter and the
    /// share-sheet type registration.
    public var supportedFileExtensions: Set<String> {
        Set(decoders.flatMap(\.fileExtensions))
    }

    /// The decoder claiming `fileExtension` (case-insensitive), if any.
    public func decoder(forFileExtension fileExtension: String) -> (any RouteFileDecoder)? {
        let key = fileExtension.lowercased()
        return decoders.first { $0.fileExtensions.contains(key) }
    }

    /// Decode `data` as a route file with this extension. An extension no decoder
    /// claims throws `FormatError.unsupportedFileType`.
    public func importRoute(from data: Data, fileExtension: String) throws -> ImportedRoute {
        guard let decoder = decoder(forFileExtension: fileExtension) else {
            throw FormatError.unsupportedFileType(fileExtension: fileExtension)
        }
        return try decoder.decode(data)
    }
}
