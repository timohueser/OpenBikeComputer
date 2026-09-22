import Foundation
import OBCDomain

/// The OBCU update-container header, decoded on the phone so a corrupt or foreign file fails in
/// the picker and never on the device. The Rust `obc-dfu` crate is the canonical writer and
/// reader; this is the app-side twin, pinned byte for byte against the same fixture.
///
/// A firmware transfer carries the whole container: this 64-byte header, the raw image, and the
/// Ed25519 signature trailer. The transfer layer stays format-blind, and the header is validated
/// here only to gate the upload.
///
/// The app does not verify the signature: the trusted key lives in the firmware, not on the phone,
/// and an app-side "valid" would mean nothing the device does not re-establish over what actually
/// landed on the card. What the app must do is carry the trailer intact, because trimming at the
/// end of the image would stage a file whose signature the device cannot find and would refuse as
/// truncated. It also refuses an unsigned container up front, because the device will.
public struct OBCUHeader: Equatable, Sendable {
    /// Fixed header length, bytes.
    public static let length = 64
    /// Magic: `OBCU`.
    public static let magic = Array("OBCU".utf8)
    /// The only header layout this app reads. It is `1` for both container generations by design,
    /// because the signature scheme, not this field, tells them apart. A different value is a hard
    /// reject, never a silent migration.
    public static let version: UInt16 = 1
    /// The signature scheme: 0 unsigned, 1 Ed25519.
    public static let sigSchemeNone: UInt16 = 0
    public static let sigSchemeEd25519: UInt16 = 1
    /// Bytes of the Ed25519 signature trailer.
    public static let sigLength = 64
    /// Byte length of the NUL-padded `fw_version` field.
    public static let fwVersionFieldLength = 32
    /// Bytes of the header covered by the header CRC, which is everything but the CRC.
    public static let headerCRCLength = 60
    /// The device's whole application slot, and so the largest raw image it can flash
    /// (`obc_dfu::MAX_IMAGE_LEN`, `OBCU_Spec.md` 1.1). An announced object past it is rejected at
    /// announce, so the app refuses it here first.
    public static let maxImageLength: UInt32 = 2_023_424

    /// Length of the raw image following the header, bytes.
    public let imageLength: UInt32
    /// CRC-32 over the raw image only, the bytes after the header.
    public let imageCRC32: UInt32
    /// The version string, trailing NULs trimmed: the value shown in the UI and, after a confirmed
    /// update, reported by the device on reconnect.
    public let fwVersion: String
    /// The signature scheme in the header's reserved space. This, not `version`, tells the
    /// container generations apart.
    public let sigScheme: UInt16
    /// Bytes of signature trailer after the image: 64 for Ed25519, 0 when unsigned.
    public let sigLength: UInt16

    /// Decode a 64-byte header, or nil for anything but a clean read of this format: bad magic,
    /// the wrong version, or a failed header CRC. A non-nil result guarantees the length and CRC
    /// fields are the ones the writer stored; the raw-image CRC is verified separately.
    public static func decode(_ bytes: Data) -> OBCUHeader? {
        guard bytes.count >= length else { return nil }
        let b = bytes.startIndex
        guard Array(bytes[b ..< b + 4]) == magic else { return nil }
        guard bytes.readUInt16LE(at: b + 4) == version else { return nil }
        let storedCRC = bytes.readUInt32LE(at: b + 60)
        guard storedCRC == CRC32.checksum(bytes[b ..< b + headerCRCLength]) else { return nil }
        let versionField = bytes[b + 16 ..< b + 16 + fwVersionFieldLength]
        let end = versionField.firstIndex(of: 0) ?? versionField.endIndex
        let fw = String(decoding: versionField[versionField.startIndex ..< end], as: UTF8.self)
        return OBCUHeader(
            imageLength: bytes.readUInt32LE(at: b + 8),
            imageCRC32: bytes.readUInt32LE(at: b + 12),
            fwVersion: fw,
            sigScheme: bytes.readUInt16LE(at: b + 48),
            sigLength: bytes.readUInt16LE(at: b + 50)
        )
    }
}

/// A firmware update the app has imported and fully validated: the whole container ready to
/// stream, plus its decoded header. Only `validate` constructs one, so a `StagedFirmware` existing
/// is proof both CRCs passed.
public struct StagedFirmware: Equatable, Sendable {
    /// The whole container: the exact payload the upload streams.
    public let container: Data
    /// The decoded 64-byte header.
    public let header: OBCUHeader

    private init(container: Data, header: OBCUHeader) {
        self.container = container
        self.header = header
    }

    /// The firmware version string, which the picker and the update screen show against the
    /// running version.
    public var version: String { header.fwVersion }
    /// The container size in bytes, which is what streams over the link.
    public var byteCount: Int { container.count }
    /// The raw application-image size in bytes (header excluded).
    public var imageByteCount: Int { Int(header.imageLength) }

    /// Validate a picked file as a firmware update: a 64-byte header whose magic, version and CRC
    /// pass, an image length within the device's slot, a signature scheme the device verifies, a
    /// file long enough to hold the header, image and signature, and a raw-image CRC matching the
    /// header's. Any failure throws a typed error the picker surfaces, so a bad download dies here
    /// and not on the device.
    ///
    /// Any bytes past the container are trailing slack and ignored: the container is trimmed to
    /// exactly its declared length, so only those bytes stream. The signature trailer is part of
    /// the container and is never trimmed, because trimming it would stage a file the device
    /// refuses as truncated.
    public static func validate(_ data: Data) throws -> StagedFirmware {
        guard data.count >= OBCUHeader.length else { throw FirmwareImageError.tooSmall }
        guard let header = OBCUHeader.decode(data.prefix(OBCUHeader.length)) else {
            throw FirmwareImageError.notOBCU
        }
        guard header.imageLength > 0, header.imageLength <= OBCUHeader.maxImageLength else {
            throw FirmwareImageError.oversize
        }
        // The device installs signed containers only, so an unsigned one, or a scheme this
        // firmware generation does not verify, is refused before the upload.
        guard header.sigScheme == OBCUHeader.sigSchemeEd25519,
              Int(header.sigLength) == OBCUHeader.sigLength
        else {
            throw FirmwareImageError.unsigned
        }
        let expected = OBCUHeader.length + Int(header.imageLength) + Int(header.sigLength)
        guard data.count >= expected else { throw FirmwareImageError.truncated }
        // Trim any trailing slack: what we stage and stream is exactly the header, raw image and
        // signature trailer.
        let container = data.prefix(expected)
        let imageStart = container.startIndex + OBCUHeader.length
        let body = container[imageStart ..< imageStart + Int(header.imageLength)]
        guard CRC32.checksum(body) == header.imageCRC32 else { throw FirmwareImageError.imageCRCMismatch }
        return StagedFirmware(container: Data(container), header: header)
    }
}

/// Why a picked file is not a usable firmware update. Each maps to one plain sentence in the
/// picker's rejection alert.
public enum FirmwareImageError: Error, Equatable, Sendable {
    /// Shorter than the 64-byte OBCU header.
    case tooSmall
    /// Not an OBCU container: bad magic, an unknown header version, or a failed header CRC.
    case notOBCU
    /// The raw image is empty, or larger than the device's update slot. The device would reject it
    /// at announce, so the app refuses it up front.
    case oversize
    /// The file is shorter than its header says, so it cannot hold the header, image and
    /// signature. Trailing bytes past the container are not an error: they are slack.
    case truncated
    /// The raw image failed its CRC-32: a corrupt download.
    case imageCRCMismatch
    /// The container carries no signature this device's firmware verifies. The device refuses to
    /// install it, so the app refuses to spend a transfer on it.
    case unsigned
}
