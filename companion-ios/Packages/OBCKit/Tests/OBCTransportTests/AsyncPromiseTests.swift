import XCTest
@testable import OBCTransport

final class AsyncPromiseTests: XCTestCase {
    func testLateReaderGetsValueImmediately() async {
        let promise = AsyncPromise<Int>()
        promise.fulfill(7)
        let got = await promise.value
        XCTAssertEqual(got, 7)
        XCTAssertEqual(promise.current, 7)
    }

    func testEarlyReadersAllResume() async {
        let promise = AsyncPromise<String>()
        XCTAssertNil(promise.current)
        async let a = promise.value
        async let b = promise.value
        promise.fulfill("done")
        let (gotA, gotB) = await (a, b)
        XCTAssertEqual(gotA, "done")
        XCTAssertEqual(gotB, "done")
    }

    func testFirstFulfillWins() async {
        let promise = AsyncPromise<Int>()
        promise.fulfill(1)
        promise.fulfill(2)   // racing terminal paths: the later one is a no-op
        let got = await promise.value
        XCTAssertEqual(got, 1)
    }
}
