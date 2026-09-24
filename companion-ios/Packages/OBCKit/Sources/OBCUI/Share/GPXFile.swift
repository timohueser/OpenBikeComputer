import CoreTransferable
import Foundation
import UniformTypeIdentifiers

/// A ride or a trip as a GPX file for the system share sheet. The encoder comes from the
/// composition root, because OBCUI does not import OBCFormats. It runs only when the rider picks
/// a destination.
public struct GPXFile: Transferable, Sendable {
    let name: String
    let encode: @Sendable () throws -> Data

    public init(name: String, encode: @escaping @Sendable () throws -> Data) {
        self.name = name
        self.encode = encode
    }

    public var fileName: String { Self.fileName(for: name) }

    /// The name as a file name that every file system accepts: path separators, reserved
    /// characters and control characters become "-", and leading dots go, so the file is never
    /// hidden. File systems cap a name at 255 bytes and a name has no length limit, so the
    /// base is cut to `maxBaseBytes` UTF-8 bytes on a character boundary.
    public static func fileName(for name: String) -> String {
        let reserved = CharacterSet(charactersIn: "/\\:?%*|\"<>").union(.controlCharacters)
        let cleaned = String(String.UnicodeScalarView(
            name.unicodeScalars.map { reserved.contains($0) ? "-" : $0 }
        ))
        let edges = CharacterSet.whitespaces.union(["."])
        var base = cleaned.trimmingCharacters(in: edges)
        while base.utf8.count > maxBaseBytes { base.removeLast() }
        base = base.trimmingCharacters(in: edges)
        return (base.isEmpty ? "Ride" : base) + ".gpx"
    }

    /// Well under the 255-byte name limit, with room for ".gpx".
    static let maxBaseBytes = 200

    public static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(exportedContentType: .gpx) { file in
            // A fresh folder per share, so two files with one name never overwrite each other.
            let folder = FileManager.default.temporaryDirectory
                .appendingPathComponent(UUID().uuidString, isDirectory: true)
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            let url = folder.appendingPathComponent(file.fileName)
            try file.encode().write(to: url, options: .atomic)
            return SentTransferredFile(url)
        }
    }
}

extension UTType {
    /// Declared in the app's Info.plist, like the import side.
    static let gpx = UTType(importedAs: "com.topografix.gpx", conformingTo: .xml)
}
