import Foundation

/// `weatherUnchanged` (spec §4.4, command 7): the conditional checks found no provider revision
/// newer than the bundle named by the request context. It finishes that request without sending the
/// bundle again and supplies a bounded manual-probe deferral for publication lag.
public enum WeatherUnchangedCommand {
    public static let commandByte: UInt8 = 7
    public static let maximumRetryAfterSeconds: UInt16 = 3_600

    public static func encode(requestID: UInt32, retryAfterSeconds: UInt16) -> Data {
        precondition(requestID != 0)
        precondition(retryAfterSeconds <= maximumRetryAfterSeconds)
        var data = Data([commandByte])
        data.append(UInt8(requestID & 0xFF))
        data.append(UInt8((requestID >> 8) & 0xFF))
        data.append(UInt8((requestID >> 16) & 0xFF))
        data.append(UInt8((requestID >> 24) & 0xFF))
        data.append(UInt8(retryAfterSeconds & 0xFF))
        data.append(UInt8((retryAfterSeconds >> 8) & 0xFF))
        return data
    }
}
