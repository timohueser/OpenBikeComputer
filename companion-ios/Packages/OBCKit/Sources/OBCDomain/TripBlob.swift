import Foundation

/// A whole trip object ready to upload (TR8) — the trip sibling of ``RouteBlob``.
/// The `payload` is the encoded trip object (`TripObjectCodec.encode` over the
/// name + the resolved, ride-ordered device stage ids); at this layer it stays
/// **opaque bytes** the transport frames without interpreting, exactly like a
/// route blob. Uploaded **last** in a whole-trip push (stages first, trip object
/// last — spec §7.7), so an interrupted push never dangles and a re-run is
/// idempotent.
public struct TripBlob: Equatable, Sendable {
    /// Display name the encoded object carries (≤ 48 UTF-8 bytes; the codec
    /// truncated it on a character boundary).
    public let name: String
    /// The stage device object ids the encoded object references, in ride order
    /// — kept for the mock's device-side catalog totals and for tests; the wire
    /// only ever moves `payload`.
    public let deviceStageIDs: [DeviceObjectID]
    /// Opaque encoded trip-object bytes — framed, not parsed, by the transport.
    public let payload: Data
    /// The device trip object id to **replace**, or `nil` for a fresh upload
    /// (the device assigns a new id from its own trip counter, `0xFFFF` = new).
    /// The adoption rule and a re-push both set it so the trip updates in place
    /// instead of duplicating (spec §4.1/§4.2, replace-by-id is cap-exempt).
    public let targetObjectID: DeviceObjectID?

    public init(
        name: String,
        deviceStageIDs: [DeviceObjectID],
        payload: Data,
        targetObjectID: DeviceObjectID? = nil
    ) {
        self.name = name
        self.deviceStageIDs = deviceStageIDs
        self.payload = payload
        self.targetObjectID = targetObjectID
    }
}
