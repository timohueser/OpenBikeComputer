import Foundation

/// Receiver-side reassembly for one framed object: validates each frame's CRC
/// **before** appending, tolerates offset-resume re-sends, and reports completion.
/// Pure byte math — used both by `BLEChannel.download` (app receiving a ride) and
/// by the tests to model the device receiving an upload.
///
/// A failing CRC throws `FramingError.crcMismatch` and the object is **not**
/// committed (`OBCProtocol.md` → *CoC framing*: "rejected, never committed").
public struct TransferAssembler {
    /// Object id of the transfer in progress (nil until the first frame).
    public private(set) var objectID: UInt16?
    /// Declared total size from the first frame's `totalLen` (nil until then).
    public private(set) var total: Int?
    /// Contiguous bytes committed so far.
    public private(set) var committed = Data()

    public init() {}

    /// Bytes assembled and CRC-validated so far — the resume anchor.
    public var committedLength: Int { committed.count }

    /// Whether the full object has arrived.
    public var isComplete: Bool { total.map { committed.count == $0 } ?? false }

    /// The reconstructed object, or nil until complete.
    public var object: Data? { isComplete ? committed : nil }

    /// Ingest one frame's `header` + `payload`. Returns `true` once the object is
    /// complete. Throws `FramingError.crcMismatch` on a bad frame, or
    /// `.truncated` if the stream skips ahead of the committed offset (a gap).
    @discardableResult
    public mutating func ingest(header: FrameHeader, payload: Data) throws -> Bool {
        try FrameCodec.verify(header, payload: payload)

        if total == nil {
            total = Int(header.totalLen)
            objectID = header.objectID
        }

        let start = Int(header.offset)
        let end = start + payload.count
        if start == committed.count {
            committed.append(payload)                       // in-order chunk
        } else if end <= committed.count {
            // Entirely within already-committed bytes → a resume re-send; ignore.
        } else if start < committed.count {
            // Straddles the resume boundary → append only the new tail.
            committed.append(payload.suffix(end - committed.count))
        } else {
            throw FramingError.truncated                    // gap ahead of committed
        }
        return isComplete
    }
}
