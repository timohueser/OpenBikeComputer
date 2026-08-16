import Foundation

/// §14.0's frame-limit derivation, as cases rather than prose: limits are derived from the link,
/// then negotiated, and they fail closed.
public enum FrameLimitNegotiation {
    public enum Outcome: Hashable, Sendable {
        /// The negotiated limit: the smallest of the transport ceiling and the two advertised
        /// maxima.
        case negotiated(Int)
        /// Below the protocol minimum no negotiation is possible; the device answers Hello (or
        /// refuses the CoC) with `resourceLimit/minimum{Control,Stream}Frame`.
        case belowProtocolMinimum
        /// Below a 64-byte frame — the 16-byte header plus a text-free ErrorBody — even the refusal
        /// is undeliverable, so the adapter disconnects rather than truncating an error.
        case undeliverable
    }

    /// §14.0: "One ATT Write Request value carries at most `ATT_MTU - 3` bytes, and so does one
    /// indication value."
    public static func bleControlCeiling(attMTU: Int) -> Int { attMTU - 3 }

    public static func control(transportCeiling: Int, clientMaximum: Int, deviceMaximum: Int)
        -> Outcome
    {
        let negotiated = min(transportCeiling, clientMaximum, deviceMaximum)
        if negotiated < WireLimits.minimumStreamFrame { return .undeliverable }
        if negotiated < WireLimits.minimumControlFrame { return .belowProtocolMinimum }
        return .negotiated(negotiated)
    }

    /// §14.0 + §14.1: the effective stream limit is `min(negotiated stream maximum, CoC SDU)`,
    /// fixed at CoC establishment; an SDU below the 64-byte floor refuses the channel.
    public static func stream(transportCeiling: Int, clientMaximum: Int, deviceMaximum: Int)
        -> Outcome
    {
        let negotiated = min(transportCeiling, clientMaximum, deviceMaximum)
        if negotiated < WireLimits.minimumStreamFrame { return .belowProtocolMinimum }
        return .negotiated(negotiated)
    }
}
