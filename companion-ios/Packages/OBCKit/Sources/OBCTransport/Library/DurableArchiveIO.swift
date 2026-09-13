import Darwin
import Foundation

/// The archive commit uses a file cache flush plus directory fsync after rename.
/// Every error is returned to the caller, including a barrier after publication.
enum DurableArchiveIO {
    static func createDirectory(_ url: URL, beneath root: URL) throws {
        try checkBoundary(url, root: root)
        var isDirectory: ObjCBool = false
        if FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory) {
            guard isDirectory.boolValue else { throw RideArchiveError.unreadableArchive }
            return
        }
        guard url.path != root.path else { throw RideArchiveError.unreadableArchive }
        let parent = url.deletingLastPathComponent()
        try createDirectory(parent, beneath: root)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: false)
        try syncDirectory(parent)
    }

    static func syncFile(_ url: URL) throws {
        let fd = open(url.path, O_RDONLY | O_NOFOLLOW)
        guard fd >= 0 else { throw posixError() }
        defer { close(fd) }
        guard fcntl(fd, F_FULLFSYNC) == 0 else { throw posixError() }
    }

    static func syncAncestors(_ url: URL, through root: URL) throws {
        try checkBoundary(url, root: root)
        var path = url.path
        while true {
            try syncDirectory(URL(fileURLWithPath: path, isDirectory: true))
            if path == root.path { return }
            path = (path as NSString).deletingLastPathComponent
        }
    }

    private static func checkBoundary(_ url: URL, root: URL) throws {
        guard url.path == root.path || url.path.hasPrefix(root.path + "/") else {
            throw RideArchiveError.unreadableArchive
        }
    }

    static func rename(_ source: URL, to destination: URL) throws {
        guard Darwin.rename(source.path, destination.path) == 0 else { throw posixError() }
    }

    private static func syncDirectory(_ url: URL) throws {
        let fd = open(url.path, O_RDONLY | O_DIRECTORY)
        guard fd >= 0 else { throw posixError() }
        defer { close(fd) }
        guard fsync(fd) == 0 else { throw posixError() }
    }

    private static func posixError() -> POSIXError {
        POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
    }
}
