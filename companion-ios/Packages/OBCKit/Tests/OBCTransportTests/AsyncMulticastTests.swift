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

    /// Ordering hammer (#364): a subscriber racing a sender must never observe
    /// a stale replay *after* a newer value. Before the fix, `stream()` yielded
    /// the replay after releasing the lock, so a `send` in that window landed
    /// first and the subscriber's sequence went new-then-stale.
    func testSubscribeRacingSendNeverYieldsStaleReplayAfterNewerValue() async {
        let multicast = AsyncMulticast<Int>(0)
        let total = 200_000
        let sender = Task.detached {
            for i in 1...total { multicast.send(i) }
            multicast.finish()
        }

        // Subscribe over and over while the sender hammers; each subscription's
        // first two elements (replay, then a live send) must be increasing —
        // a stale replay delivered after a newer value inverts them.
        var racedSubscriptions = 0
        while multicast.value < total {
            var iterator = multicast.stream().makeAsyncIterator()
            var previous: Int?
            for _ in 0..<4 {  // replay + a few live sends is where a stale replay surfaces
                guard let value = await iterator.next() else { break }
                if let previous {
                    XCTAssertGreaterThan(
                        value, previous,
                        "stale value (\(value)) delivered after newer value (\(previous))"
                    )
                    racedSubscriptions += 1
                }
                previous = value
            }
        }
        await sender.value
        // Sanity: the loop genuinely subscribed mid-stream, not just at the end.
        XCTAssertGreaterThan(racedSubscriptions, 50, "hammer never raced the sender")
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
