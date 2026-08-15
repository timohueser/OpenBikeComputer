import Foundation
import Testing
@testable import OBCTransport

/// The S4-flagged follow-up (epic #638): the `routeList` decoder must be
/// **`entry_len`-driven**, not hard-coded — a pre-expiry 76-byte device, the
/// 84-byte v2+expiry device, and a hypothetical longer future entry all decode.
/// The byte-exact 84-byte fixture round-trip lives in `ProtocolVectorTests`; this
/// suite pins the entry-length matrix the fixture can't cover alone.
@Suite struct RouteListRetentionTests {
    /// A representative entry with a full expiry tail (`expires_at`, retention).
    private var entry: RouteListEntry {
        RouteListEntry(
            objectID: 7, byteLen: 308, distanceMeters: 2207, ascentMeters: 76,
            pointCount: 9, waypointCount: 2, name: "Vector Loop",
            crc32: 0x1BFB_6E3C, expiresAt: 1_784_808_000, retention: 3)
    }

    /// Assemble a `routeList` object with an explicit header `entry_len` and
    /// per-entry byte payloads (already sized to `entryLen`).
    private func list(entryLen: Int, entries: [Data]) -> Data {
        var data = Data([2, UInt8(entryLen)])
        let count = UInt16(entries.count)
        data.append(UInt8(count & 0xFF)); data.append(UInt8(count >> 8))
        data.append(UInt8(count & 0xFF)); data.append(UInt8(count >> 8))  // total = count
        for entry in entries { data.append(entry) }
        return data
    }

    /// A **76-byte** entry (a pre-expiry device, `entry_len == 76`): the v2 core
    /// decodes, both tail fields are `nil`.
    @Test func preExpiryEntryHasNilTail() throws {
        let core = entry.encode().prefix(RouteListEntry.coreLength)  // drop the 8-byte tail
        let decoded = try RouteList.decode(list(entryLen: 76, entries: [Data(core)]))
        #expect(decoded.count == 1)
        #expect(decoded[0].objectID == 7)
        #expect(decoded[0].crc32 == 0x1BFB_6E3C)
        #expect(decoded[0].expiresAt == nil)
        #expect(decoded[0].retention == nil)
    }

    /// An **84-byte** entry (`entry_len == 84`): the tail fills.
    @Test func expiryEntryFillsTheTail() throws {
        let decoded = try RouteList.decode(list(entryLen: 84, entries: [entry.encode()]))
        #expect(decoded[0].expiresAt == 1_784_808_000)
        #expect(decoded[0].retention == 3)
    }

    /// A **longer** future entry (`entry_len == 90`, 6 unknown trailing bytes):
    /// the known 76-byte core + the 84-byte tail decode, the extra is skipped
    /// cleanly, and the next entry still lands at `6 + 90·k`.
    @Test func longerEntrySkipsItsTailCleanly() throws {
        let padded = entry.encode() + Data(count: 6)  // 90 bytes
        var second = entry
        second.objectID = 8
        second.retention = 1
        second.expiresAt = nil
        let decoded = try RouteList.decode(
            list(entryLen: 90, entries: [padded, second.encode() + Data(count: 6)]))
        #expect(decoded.count == 2)
        #expect(decoded[0].objectID == 7)
        #expect(decoded[0].expiresAt == 1_784_808_000)
        #expect(decoded[0].retention == 3)
        #expect(decoded[1].objectID == 8)
        #expect(decoded[1].expiresAt == nil)  // wire 0 → nil
        #expect(decoded[1].retention == 1)
    }

    /// A wire `expires_at` of `0` (never / countdown not started) decodes to
    /// `nil`, and re-encodes back to `0` — the round-trip the fixture relies on.
    @Test func zeroExpiresAtRoundTripsAsNil() throws {
        var e = entry
        e.expiresAt = nil
        e.retention = 0
        let decoded = try RouteList.decode(list(entryLen: 84, entries: [e.encode()]))
        #expect(decoded[0].expiresAt == nil)
        #expect(decoded[0].retention == 0)
        // Re-encode writes 84-byte entries with a zeroed expiry.
        let reencoded = try RouteList.decode(RouteList.encode(decoded))
        #expect(reencoded[0].expiresAt == nil)
    }

    /// A header advertising an `entry_len` **below** the 76-byte v2 core is
    /// rejected — a route codec can't decode a slot too small to hold its core.
    @Test func headerBelowCoreLengthIsRejected() {
        let bogus = list(entryLen: 72, entries: [Data(count: 72)])
        #expect(throws: (any Error).self) { try RouteList.decode(bogus) }
    }
}
