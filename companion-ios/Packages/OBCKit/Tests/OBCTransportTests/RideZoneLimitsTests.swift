import Testing
import OBCDomain

/// The zone edges mirror the device's `effort.rs` cases, so a ride shows the zones it showed on
/// the device.
struct RideZoneLimitsTests {
    private let limits = RideZoneLimits(maxHeartRate: 200, ftpWatts: 200)

    @Test func heartRateEdgesShareTheirEndsUpward() {
        // Max HR 200: Z2 from 120, Z3 from 140, Z4 from 160, Z5 from 180.
        let zones = [119, 120, 139, 140, 159, 160, 179, 180].map { limits.heartRateZone(bpm: $0) }
        #expect(zones == [0, 1, 1, 2, 2, 3, 3, 4])
    }

    @Test func powerEdgesAboveZ2AreExclusive() {
        // FTP 200: Z2 from 55 %, Z3 above 75 %, Z4 above 90 %, Z5 above 105 %.
        let zones = [109, 110, 150, 151, 180, 181, 210, 211].map { limits.powerZone(watts: $0) }
        #expect(zones == [0, 1, 1, 2, 2, 3, 3, 4])
        #expect(RideZoneLimits(maxHeartRate: nil, ftpWatts: 250).powerZone(watts: 264) == 4,
                "105.6 % is Z5, with no percent truncated away")
    }

    @Test func aLimitNotSetHasNoZones() {
        #expect(RideZoneLimits.notSet.heartRateZone(bpm: 150) == nil)
        #expect(RideZoneLimits.notSet.powerZone(watts: 200) == nil)
        #expect(RideZoneLimits(maxHeartRate: 185, ftpWatts: nil).heartRateZone(bpm: 140) == 2)
    }
}
