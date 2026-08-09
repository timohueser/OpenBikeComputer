import Foundation
import Testing
@testable import OBCWeatherWire

struct OBCWeatherCodecTests {
    private static let vectorsDirectory = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent() // OBCWeatherWireTests
        .deletingLastPathComponent() // Tests
        .deletingLastPathComponent() // OBCKit
        .deletingLastPathComponent() // Packages
        .deletingLastPathComponent() // companion-ios
        .deletingLastPathComponent() // repository root
        .appendingPathComponent("specs/vectors")

    private func fixture(_ name: String) throws -> Data {
        let url = Self.vectorsDirectory.appendingPathComponent(name)
        return try #require(FileManager.default.contents(atPath: url.path), "missing fixture \(name)")
    }

    private func readUInt32(_ data: Data, at offset: Int) -> UInt32 {
        UInt32(data[offset])
            | UInt32(data[offset + 1]) << 8
            | UInt32(data[offset + 2]) << 16
            | UInt32(data[offset + 3]) << 24
    }

    private func crc32TreatingWeatherFieldAsZero(_ data: Data) -> UInt32 {
        var crc: UInt32 = 0xFFFF_FFFF
        for (index, storedByte) in data.enumerated() {
            let byte: UInt8 = (88..<92).contains(index) ? 0 : storedByte
            crc ^= UInt32(byte)
            for _ in 0..<8 { crc = crc & 1 == 1 ? 0xEDB8_8320 ^ (crc >> 1) : crc >> 1 }
        }
        return crc ^ 0xFFFF_FFFF
    }

    private func refreshCRC(_ data: inout Data) {
        data[88..<92] = Data(repeating: 0, count: 4)
        let crc = crc32TreatingWeatherFieldAsZero(data)
        for byte in 0..<4 { data[88 + byte] = UInt8(truncatingIfNeeded: crc >> (byte * 8)) }
    }

    @Test
    func positiveGoldenVectorsDecodeAndReencodeByteExactly() throws {
        let names = [
            "weather-minimal-dry.obcw", "weather-dwd-96x96-9f.obcw",
            "weather-coarse-model.obcw", "weather-nodata-tile.obcw",
            "weather-raw-tile.obcw", "weather-rle-tile.obcw", "weather-max-policy.obcw",
        ]
        for name in names {
            let bytes = try fixture(name)
            let decoded = try OBCWeatherCodec.decode(bytes)
            #expect(try OBCWeatherCodec.encodeFormat(decoded) == bytes, "Swift drift for \(name)")
            #expect(try OBCWeatherCodec.encode(decoded) == bytes, "producer-policy drift for \(name)")
        }
        let dry = try OBCWeatherCodec.decode(fixture("weather-minimal-dry.obcw"))
        #expect(dry.hourly.count == 24)
        #expect(dry.rainFrames.isEmpty)
        let dwdBytes = try fixture("weather-dwd-96x96-9f.obcw")
        let dwd = try OBCWeatherCodec.decode(dwdBytes)
        #expect(dwdBytes.count == 46_480)
        #expect(dwd.rainFrames.count == 9)
        #expect(dwd.rainFrames.allSatisfy { $0.width == 96 && $0.height == 96 && $0.tiles.count == 36 })
        #expect(try fixture("weather-max-policy.obcw").count == OBCWeatherCodec.producerPolicyMaximumLength)
    }

    @Test
    func malformedGoldenVectorsAreRejected() throws {
        let names = [
            "weather-invalid-truncated.obcw", "weather-invalid-bad-offset.obcw",
            "weather-invalid-overlap.obcw", "weather-invalid-hourly-flags.obcw",
            "weather-invalid-hourly-reserved.obcw", "weather-invalid-nibble.obcw",
            "weather-invalid-rle-overlong.obcw", "weather-invalid-rle-noncanonical.obcw",
            "weather-invalid-crc.obcw",
            "weather-invalid-time-order.obcw",
        ]
        for name in names {
            let bytes = try fixture(name)
            #expect(throws: (any Error).self, "accepted \(name)") { try OBCWeatherCodec.decode(bytes) }
        }
    }

    @Test
    func decoderNeverTrapsOnArbitraryBytes() {
        var state: UInt32 = 0xC0DE_1187
        for length in 0..<1_024 {
            var bytes = Data(); bytes.reserveCapacity(length)
            for _ in 0..<length {
                state ^= state << 13; state ^= state >> 17; state ^= state << 5
                bytes.append(UInt8(truncatingIfNeeded: state))
            }
            _ = try? OBCWeatherCodec.decode(bytes)
        }
    }

    @Test
    func validCRCStructuredMutationsAndEveryTruncationReachRealParserBoundaries() throws {
        let compact = try fixture("weather-rle-tile.obcw")
        for length in 0..<compact.count {
            #expect((try? OBCWeatherCodec.decode(Data(compact.prefix(length)))) == nil)
        }

        let seed = try fixture("weather-dwd-96x96-9f.obcw")
        let headerLength = 112, hourlyLength = 24 * 24, descriptorLength = 9 * 48
        let frameBase = headerLength + hourlyLength
        let firstDirectory = Int(readUInt32(seed, at: frameBase + 16))
        let firstPayload = Int(readUInt32(seed, at: frameBase + 24))
        let ranges = [
            (headerLength, frameBase, "hourly"),
            (frameBase, frameBase + descriptorLength, "frame descriptors"),
            (firstDirectory, firstPayload, "tile directory"),
            (firstPayload, firstPayload + 36 * 128, "tile payload"),
        ]
        for (start, end, label) in ranges {
            var rejected = 0
            for mutation in 0..<128 {
                var bytes = seed
                let offset = start + (mutation * 97) % (end - start)
                bytes[offset] ^= UInt8(1 << (mutation % 8))
                refreshCRC(&bytes)
                #expect(readUInt32(bytes, at: 88) == crc32TreatingWeatherFieldAsZero(bytes), "CRC drift in \(label)")
                if (try? OBCWeatherCodec.decode(bytes)) == nil { rejected += 1 }
            }
            #expect(rejected > 0, "structured \(label) mutations never exercised a rejection path")
        }
    }

    @Test
    func followingHourBoundaryAndOffsetArePinned() throws {
        var bundle = try OBCWeatherCodec.decode(fixture("weather-minimal-dry.obcw"))
        bundle.validUntilUnixSeconds = bundle.validFromUnixSeconds + 24 * 3_600
        #expect(try OBCWeatherCodec.decode(OBCWeatherCodec.encodeFormat(bundle)) == bundle)

        bundle.validUntilUnixSeconds -= 1
        #expect(throws: OBCWeatherWireError.malformed) { try OBCWeatherCodec.encodeFormat(bundle) }

        bundle = try OBCWeatherCodec.decode(fixture("weather-minimal-dry.obcw"))
        bundle.hourly[0].validTimeOffsetSeconds = 3_600
        #expect(throws: OBCWeatherWireError.malformed) { try OBCWeatherCodec.encodeFormat(bundle) }
    }

    @Test
    func producerPolicyIsSeparateFromFormatCapacity() throws {
        var bundle = try OBCWeatherCodec.decode(fixture("weather-max-policy.obcw"))
        let tile = [UInt8](repeating: 0, count: 256)
        bundle.rainFrames.append(OBCWeatherRainFrame(
            validAtUnixSeconds: bundle.rainFrames[0].validAtUnixSeconds + 900,
            width: 16, height: 16, cellSizeMetres: 1_000, quality: .forecast, tiles: [tile]))
        let formatBytes = try OBCWeatherCodec.encodeFormat(bundle)
        #expect(formatBytes.count > OBCWeatherCodec.producerPolicyMaximumLength)
        #expect(throws: OBCWeatherWireError.producerPolicyExceeded) { try OBCWeatherCodec.encode(bundle) }
        #expect(try OBCWeatherCodec.decode(formatBytes) == bundle)
    }

    @Test
    func encoderRejectsImpossibleDimensionsAndNonNoDataPadding() throws {
        var bundle = try OBCWeatherCodec.decode(fixture("weather-raw-tile.obcw"))
        bundle.rainFrames[0].width = 17
        #expect(throws: OBCWeatherWireError.malformed) { try OBCWeatherCodec.encodeFormat(bundle) }

        bundle = try OBCWeatherCodec.decode(fixture("weather-raw-tile.obcw"))
        bundle.rainFrames[0].width = 15
        #expect(throws: OBCWeatherWireError.malformed) { try OBCWeatherCodec.encodeFormat(bundle) }
    }
}
