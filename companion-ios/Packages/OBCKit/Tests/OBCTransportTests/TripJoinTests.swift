import Testing
import Foundation
import OBCDomain

/// The order proposal for several files and the joins between them.
struct TripJoinTests {
    private func coordinate(_ km: Double) -> Coordinate {
        Coordinate(latitude: 46.5, longitude: 8 + km * 1000 / (111_320 * cos(46.5 * Double.pi / 180)))
    }

    private func file(_ name: String, _ from: Double, _ to: Double) -> TripJoin.File {
        TripJoin.File(name: name, start: coordinate(from), end: coordinate(to))
    }

    @Test
    func filesInTheWrongOrderChainByTheirEnds() {
        let files = [file("Grimsel", 20, 40), file("Brig", 40, 60), file("Andermatt", 0, 20)]
        #expect(TripJoin.proposedOrder(files) == [2, 0, 1])
    }

    @Test
    func aGapDoesNotBreakTheChain() {
        let files = [file("c", 43.4, 60), file("a", 0, 20), file("b", 20, 40)]
        #expect(TripJoin.proposedOrder(files) == [1, 2, 0])
    }

    @Test
    func filenameOrderWinsWhenTheEndsSayNothing() {
        // Three loops from one town: every order joins equally well.
        let loops = [file("Stage 10", 0, 0), file("Stage 2", 0, 0), file("Stage 1", 0, 0)]
        #expect(TripJoin.proposedOrder(loops) == [2, 1, 0])
        // Unrelated files far apart: the chain is no better than the names.
        let apart = [file("B", 100, 110), file("A", 0, 10)]
        #expect(TripJoin.proposedOrder(apart) == [1, 0])
    }

    @Test
    func gapsMeasureEveryBoundary() {
        let gaps = TripJoin.gaps([file("a", 0, 20), file("b", 20.05, 40), file("c", 43.4, 60)])
        #expect(gaps.count == 2)
        #expect(gaps[0] < Trip.transferMinMeters)
        #expect(abs(gaps[1] - 3_400) < 5)
    }
}
