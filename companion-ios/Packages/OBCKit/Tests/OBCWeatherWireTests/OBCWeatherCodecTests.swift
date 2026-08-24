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

    /// Bit-by-bit CRC-32, independent of the table-driven production implementation, used once to
    /// pin it. Too slow for a loop: ~57 ms per pass over the 46 KB seed in a debug build.
    private func crc32TreatingWeatherFieldAsZero(_ data: Data) -> UInt32 {
        var crc: UInt32 = 0xFFFF_FFFF
        for (index, storedByte) in data.enumerated() {
            let byte: UInt8 = (88..<92).contains(index) ? 0 : storedByte
            crc ^= UInt32(byte)
            for _ in 0..<8 { crc = crc & 1 == 1 ? 0xEDB8_8320 ^ (crc >> 1) : crc >> 1 }
        }
        return crc ^ 0xFFFF_FFFF
    }

    /// Writes the checksum the decoder recomputes, over the same three spans it hashes.
    private func refreshCRC(_ data: inout Data) {
        var hasher = CRC32.Hasher()
        hasher.update(data.prefix(88))
        hasher.update(Data(repeating: 0, count: 4))
        hasher.update(data.dropFirst(92))
        let crc = hasher.finalize()
        for byte in 0..<4 { data[88 + byte] = UInt8(truncatingIfNeeded: crc >> (byte * 8)) }
    }

    @Test
    func weatherWireCRC32MatchesTheIndependentImplementationAndCanonicalCheckValue() throws {
        #expect(CRC32.checksum(Data("123456789".utf8)) == 0xCBF4_3926)
        var bytes = try fixture("weather-dwd-96x96-9f.obcw")
        refreshCRC(&bytes)
        #expect(readUInt32(bytes, at: 88) == crc32TreatingWeatherFieldAsZero(bytes))
    }

    @Test
    func positiveGoldenVectorsDecodeAndReencodeByteExactly() throws {
        let names = [
            "weather-minimal-dry.obcw", "weather-dwd-96x96-9f.obcw",
            "weather-coarse-model.obcw", "weather-nodata-tile.obcw",
            "weather-latent-observation.obcw",
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

        let latent = try OBCWeatherCodec.decode(fixture("weather-latent-observation.obcw"))
        #expect(latent.rainFrames[0].validAtUnixSeconds == latent.validFromUnixSeconds - 4 * 3_600)
    }

    @Test
    func malformedGoldenVectorsAreRejected() throws {
        let names = [
            "weather-invalid-truncated.obcw", "weather-invalid-bad-offset.obcw",
            "weather-invalid-overlap.obcw", "weather-invalid-hourly-flags.obcw",
            "weather-invalid-hourly-reserved.obcw", "weather-invalid-nibble.obcw",
            "weather-invalid-raw-compressible.obcw",
            "weather-invalid-rle-overlong.obcw", "weather-invalid-rle-noncanonical.obcw",
            "weather-invalid-crc.obcw",
            "weather-invalid-time-order.obcw",
            "weather-invalid-frame-nonpositive.obcw",
            "weather-invalid-frame-after-valid-until.obcw",
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
    func sharedPrecipitationCodecPinsQuantizationAndCanonicalTiles() throws {
        let boundaries: [(Double, UInt8)] = [
            (0, 0), (Double.leastNonzeroMagnitude, 1), (0.10, 2), (0.25, 3),
            (0.50, 4), (1, 5), (2, 6), (4, 7), (6, 8), (10, 9),
            (16, 10), (25, 11), (50, 12), (500, 12),
        ]
        for (rate, expected) in boundaries {
            #expect(OBCPrecipitationTileCodec.quantize(rateMillimetresPerHour: rate) == expected)
        }
        for rate in [-1.0, Double.nan, Double.infinity, -Double.infinity] {
            #expect(OBCPrecipitationTileCodec.quantize(rateMillimetresPerHour: rate) == 15)
        }

        let raw = (0..<256).map { UInt8($0 % 13) }
        let compressed = [UInt8](repeating: 6, count: 256)
        for (tile, codec, length) in [(raw, UInt8(0), 128), (compressed, UInt8(1), 16)] {
            let encoded = try OBCPrecipitationTileCodec.encode(tile)
            #expect(encoded.codec == codec)
            #expect(encoded.bytes.count == length)
            #expect(try OBCPrecipitationTileCodec.decode(codec: encoded.codec, encoded: encoded.bytes) == tile)
        }
        #expect(throws: OBCWeatherWireError.malformed) {
            try OBCPrecipitationTileCodec.encode([UInt8](repeating: 13, count: 256))
        }
        #expect(throws: OBCWeatherWireError.malformed) {
            try OBCPrecipitationTileCodec.decode(codec: 0, encoded: Data(repeating: 0, count: 128))
        }

        var state: UInt32 = 0x1187_0BC5
        for _ in 0..<256 {
            let tile: [UInt8] = (0..<256).map { _ in
                state ^= state << 13; state ^= state >> 17; state ^= state << 5
                let candidate = UInt8(state & 0x0F)
                return OBCPrecipitationTileCodec.validIntensity(candidate) ? candidate : 15
            }
            let encoded = try OBCPrecipitationTileCodec.encode(tile)
            #expect(try OBCPrecipitationTileCodec.decode(codec: encoded.codec, encoded: encoded.bytes) == tile)
        }
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
