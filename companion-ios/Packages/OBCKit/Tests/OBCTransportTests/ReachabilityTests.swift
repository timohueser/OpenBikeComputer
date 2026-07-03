import XCTest
import OBCTransport

/// The reachability seam behind the MapKit basemap. Only `ConstantReachability`
/// is host-testable (the `NWPathMonitor` conformer needs a real network stack);
/// it's the one that also backs the `-OBCNetwork` launch override.
final class ReachabilityTests: XCTestCase {
    func testConstantReachabilityReplaysItsValue() async {
        for value in [true, false] {
            let reachability = ConstantReachability(value)
            var first: Bool?
            for await online in reachability.updates {
                first = online
                break  // replays the current value immediately, then holds
            }
            XCTAssertEqual(first, value)
        }
    }
}
