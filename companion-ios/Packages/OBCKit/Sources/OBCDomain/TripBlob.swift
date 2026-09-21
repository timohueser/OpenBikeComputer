import Foundation

/// A whole trip object ready to upload: the trip sibling of ``RouteBlob``. The `payload` is
/// opaque bytes the transport frames without interpreting. A whole-trip push sends the stages
/// first and the trip object last, so an interrupted push never dangles and a re-run is
/// idempotent.
public struct TripBlob: Equatable, Sendable {
    /// Display name the encoded object carries, at most 48 UTF-8 bytes.
    public let name: String
    /// The stage device object ids the encoded object references, in ride order. The wire only
    /// ever moves `payload`.
    public let deviceStageIDs: [DeviceObjectID]
    public let payload: Data
    /// The device trip object id to replace, or `nil` for a fresh upload, where the device
    /// assigns a new id (`0xFFFF` means new). A re-push sets it, so the trip updates in place
    /// instead of duplicating.
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
