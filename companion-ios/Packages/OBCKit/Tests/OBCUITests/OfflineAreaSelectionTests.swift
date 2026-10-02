import OBCDomain
import OBCPlanner
import Testing
@testable import OBCUI

struct OfflineAreaSelectionTests {
    @Test func viewportBoundsArePreservedUntilACornerMoves() throws {
        let viewport = [7.45, 47.5, 8.25, 48.1]
        var selection = try #require(OfflineAreaSelection(viewport))
        #expect(selection.bounds == viewport)
        selection.move(.northwest, to: Coordinate(latitude: 48.3, longitude: 7.2))
        #expect(selection.bounds == [7.2, 47.5, 8.25, 48.3])
        selection.move(.southeast, to: Coordinate(latitude: 47.8, longitude: 8.0))
        #expect(selection.bounds == [7.2, 47.8, 8.0, 48.3])
    }

    @Test(arguments: OfflineAreaSelection.Corner.allCases)
    func cornersCannotCrossOrLeaveTheWorld(corner: OfflineAreaSelection.Corner) throws {
        var selection = try #require(OfflineAreaSelection([7, 47, 9, 49]))
        selection.move(corner, to: Coordinate(latitude: corner == .northwest ? -90 : 90,
                                              longitude: corner == .northwest ? 190 : -190))
        #expect(OfflineMap.valid(selection.bounds))
        #expect(corner == .northwest
            ? selection.bounds[1] == 47 && selection.bounds[2] == 9
            : selection.bounds[0] == 7 && selection.bounds[3] == 49)
        selection.move(corner, to: Coordinate(latitude: corner == .northwest ? 90 : -90,
                                              longitude: corner == .northwest ? -190 : 190))
        #expect(OfflineMap.valid(selection.bounds))
        #expect(corner == .northwest
            ? selection.bounds == [-180, 47, 9, 85]
            : selection.bounds == [7, -85, 180, 49])
        let valid = selection
        selection.move(corner, to: Coordinate(latitude: .nan, longitude: .infinity))
        #expect(selection == valid)
    }
}
