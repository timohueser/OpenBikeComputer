import XCTest
@testable import OBCTransport

/// `state`/`battery` replay semantics (epic #234: "replay latest so a late
/// subscriber gets the current value").
final class AsyncMulticastTests: XCTestCase {
    func testLateSubscriberGetsLatestThenLiveUpdates() async {
        let multicast = AsyncMulticast<Int>(0)
        let stream = multicast.stream()  // subscription registers synchronously here
        multicast.send(1)
        multicast.send(2)

        var iterator = stream.makeAsyncIterator()
        let replay = await iterator.next()
        let first = await iterator.next()
        let second = await iterator.next()
        XCTAssertEqual(replay, 0)   // latest-at-subscribe is replayed
        XCTAssertEqual(first, 1)
        XCTAssertEqual(second, 2)
        XCTAssertEqual(multicast.value, 2)
    }

    func testSubscriberAfterSendReplaysCurrentValue() async {
        let multicast = AsyncMulticast<String>("a")
        multicast.send("b")
        var iterator = multicast.stream().makeAsyncIterator()
        let replayed = await iterator.next()
        XCTAssertEqual(replayed, "b")  // not "a"
    }

    func testFinishReplaysThenTerminates() async {
        let multicast = AsyncMulticast<Int>(9)
        multicast.finish()
        var iterator = multicast.stream().makeAsyncIterator()
        let replayed = await iterator.next()
        let terminated = await iterator.next()
        XCTAssertEqual(replayed, 9)     // one replayed value
        XCTAssertNil(terminated)        // then finished
    }

    func testFanOutToMultipleSubscribers() async {
        let multicast = AsyncMulticast<Int>(0)
        let a = multicast.stream()
        let b = multicast.stream()
        multicast.send(42)

        var ia = a.makeAsyncIterator(); var ib = b.makeAsyncIterator()
        _ = await ia.next(); _ = await ib.next()          // drop the replayed 0
        let va = await ia.next(); let vb = await ib.next()
        XCTAssertEqual(va, 42)
        XCTAssertEqual(vb, 42)
    }
}
