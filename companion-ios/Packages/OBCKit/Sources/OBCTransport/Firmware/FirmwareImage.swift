import Foundation
import OBCDomain

/// The **OBCU update-container header** (`OBCU_Spec.md` §1.1), decoded on the
/// phone so a corrupt or foreign file fails in the picker — never on the device.
/// The Rust `obc-dfu` crate is the canonical writer/reader; this is the app-side
/// twin, pinned byte-for-byte against the same `specs/vectors/` fixture.
///
/// A `fwImage` transfer (spec §7.6) carries the *whole* container — this 64-byte
/// header plus the raw image after it — so the transfer layer stays format-blind;
/// the header is validated here only to gate the upload.
public struct OBCUHeader: Equatable, Sendable {
    /// Fixed header length, bytes.
    public static let length = 64
    /// Magic — `OBCU` (OpenBikeComputer Update).
    public static let magic = Array("OBCU".utf8)
    /// The only header layout this app reads. A different version is a hard reject
    /// (settings-codec convention), never a silent migration.
    public static let version: UInt16 = 1
    /// Byte length of the NUL-padded `fw_version` field.
    public static let fwVersionFieldLength = 32
    /// Bytes of the header covered by `header_crc32` (everything but the CRC).
    public static let headerCRCLength = 60
    /// The largest raw image the device's slot can flash (`OBCU_Spec.md` §1.1
    /// `MAX_IMAGE_LEN`, the L15 DK slot minus margin) — an announced object past
    /// it is rejected at announce (spec §7.6), so the app refuses it here first.
    public static let maxImageLength: UInt32 = 1_480_000

    /// Length of the raw image following the header, bytes.
    public let imageLength: UInt32
    /// CRC-32/IEEE over the raw image only (the bytes after the header).
    public let imageCRC32: UInt32
    /// `git describe` version, trailing NULs trimmed — the value shown in the UI
    /// and, after a confirmed update, reported by DIS 0x2A26 on reconnect.
    public let fwVersion: String

    /// Decode a 64-byte header, or `nil` for anything but a clean read of *this*
    /// format — bad magic, the wrong version, or a failed header CRC (over bytes
    /// `0..60`). `Some` guarantees the length/CRC fields are the ones the writer
    /// stored; the raw-image CRC is verified separately, against the body.
    public static func decode(_ bytes: Data) -> OBCUHeader? {
        guard bytes.count >= length else { return nil }
        let b = bytes.startIndex
        guard Array(bytes[b ..< b + 4]) == magic else { return nil }
        guard bytes.readLE(at: b + 4) as UInt16 == version else { return nil }
        let storedCRC: UInt32 = bytes.readLE(at: b + 60)
        guard storedCRC == CRC32.checksum(bytes[b ..< b + headerCRCLength]) else { return nil }
        let versionField = bytes[b + 16 ..< b + 16 + fwVersionFieldLength]
        let end = versionField.firstIndex(of: 0) ?? versionField.endIndex
        let fw = String(decoding: versionField[versionField.startIndex ..< end], as: UTF8.self)
        return OBCUHeader(
            imageLength: bytes.readLE(at: b + 8),
            imageCRC32: bytes.readLE(at: b + 12),
            fwVersion: fw
        )
    }
}

/// A firmware update the app has imported and fully validated — the whole OBCU
/// container ready to stream as a `fwImage` (spec §7.6), plus its decoded header.
/// Only [`validate`](StagedFirmware/validate(_:)) constructs one, so a
/// `StagedFirmware` existing is proof both CRCs passed.
public struct StagedFirmware: Equatable, Sendable {
    /// The whole `UPDATE.BIN` — the exact `fwImage` payload (header + raw image).
    public let container: Data
    /// The decoded 64-byte header.
    public let header: OBCUHeader

    private init(container: Data, header: OBCUHeader) {
        self.container = container
        self.header = header
    }

    /// The firmware version string (`git describe`) — the picker + update screen
    /// show this against the running version.
    public var version: String { header.fwVersion }
    /// The container size in bytes (what streams over the link).
    public var byteCount: Int { container.count }
    /// The raw application-image size in bytes (header excluded).
    public var imageByteCount: Int { Int(header.imageLength) }

    /// Validate a picked file as a firmware update: a 64-byte OBCU header whose
    /// magic/version/header-CRC pass, an `image_len` within the device's slot, a
    /// file long enough to hold header + image, and a raw-image CRC matching the
    /// header's. Any failure throws a typed [`FirmwareImageError`] the picker
    /// surfaces — the point is that a bad download dies here, not on the device.
    ///
    /// Any bytes past `64 + image_len` are **FAT-cluster slack** and ignored, per
    /// `OBCU_Spec.md` §1.1 (the firmware armer conforms): the container is trimmed
    /// to exactly `64 + image_len`, so only those bytes stream and the slack never
    /// reaches the CRC or the wire. `.truncated` is kept for the genuinely-short
    /// case — a file that can't even hold header + image (DR5, #733).
    public static func validate(_ data: Data) throws -> StagedFirmware {
        guard data.count >= OBCUHeader.length else { throw FirmwareImageError.tooSmall }
        guard let header = OBCUHeader.decode(data.prefix(OBCUHeader.length)) else {
            throw FirmwareImageError.notOBCU
        }
        guard header.imageLength > 0, header.imageLength <= OBCUHeader.maxImageLength else {
            throw FirmwareImageError.oversize
        }
        let expected = OBCUHeader.length + Int(header.imageLength)
        guard data.count >= expected else { throw FirmwareImageError.truncated }
        // Trim any trailing slack: the container we stage/stream is exactly the
        // header + raw image, nothing past it.
        let container = data.prefix(expected)
        let body = container.suffix(from: container.startIndex + OBCUHeader.length)
        guard CRC32.checksum(body) == header.imageCRC32 else { throw FirmwareImageError.imageCRCMismatch }
        return StagedFirmware(container: Data(container), header: header)
    }
}

/// Why a picked file isn't a usable firmware update. Each maps to one plain
/// sentence in the picker's rejection alert.
public enum FirmwareImageError: Error, Equatable, Sendable {
    /// Shorter than the 64-byte OBCU header.
    case tooSmall
    /// Not an OBCU container: bad magic, an unknown header version, or a failed
    /// header CRC.
    case notOBCU
    /// The raw image is empty or larger than the device's update slot — the
    /// device would reject it at announce, so the app refuses it up front.
    case oversize
    /// The file is shorter than its header says — it can't hold header + image
    /// (a torn download). Trailing bytes past `64 + image_len` are *not* an error
    /// — they're FAT-cluster slack and ignored (spec §1.1).
    case truncated
    /// The raw image failed its CRC-32 — a corrupt download.
    case imageCRCMismatch
}

extension FirmwareInstallResult {
    /// Map a device `commandResult` status (spec §4.3) for the `installFw`
    /// command onto the request outcome. The device answers from cheaply-knowable
    /// edge state only (§4.4): the reference firmware never returns `error`
    /// (`invalid`) here — a bad image surfaces on the confirm card instead.
    public init(commandStatus status: CommandResult.Status) {
        switch status {
        case .ok: self = .accepted
        case .notFound: self = .noStaged
        case .busy: self = .busy
        case .error: self = .rejected
        case .unknownCommand: self = .unsupported
        }
    }
}

// MARK: - Little-endian reads (mirrors TransferDescriptor's private helpers)

extension Data {
    fileprivate func readLE(at index: Index) -> UInt16 {
        UInt16(self[index]) | (UInt16(self[index + 1]) << 8)
    }

    fileprivate func readLE(at index: Index) -> UInt32 {
        UInt32(self[index]) | (UInt32(self[index + 1]) << 8)
            | (UInt32(self[index + 2]) << 16) | (UInt32(self[index + 3]) << 24)
    }
}
