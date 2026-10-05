import Foundation
import OBCDomain
import Testing
@testable import OBCPlanner

/// The Swift half of the shared route answer vector.
struct RouteAnswerTests {
    private struct Vector: Decodable {
        struct Source: Decodable {
            struct Leg: Decodable { let from_index: Int; let to_index: Int; let totals: Totals }
            struct Totals: Decodable { let distance_m: Double; let ascent_m: Double; let seconds: Double }
            let package: String
            let profile: String
            let geometry: [[Double]]
            let elevation: [Double?]
            let elapsed: [Double]
            let legs: [Leg]
            let totals: Totals
        }
        struct Answer: Decodable { let routes: [RouteAnswer] }
        let routes: [Source]
        let answer: Answer
    }

    @Test func decodesTheSharedVectorWithinTheSpecifiedPrecision() throws {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // OBCPlannerTests
            .deletingLastPathComponent()  // Tests
            .deletingLastPathComponent()  // OBCKit
            .deletingLastPathComponent()  // Packages
            .deletingLastPathComponent()  // companion-ios
            .deletingLastPathComponent()  // repository root
            .appendingPathComponent("specs/vectors/route-answer.json")
        let vector = try JSONDecoder().decode(Vector.self, from: Data(contentsOf: url))
        #expect(vector.answer.routes.count == vector.routes.count)
        for (route, source) in zip(vector.answer.routes, vector.routes) {
            #expect(route.package == source.package && route.profile == source.profile)
            #expect(route.coordinates.count == source.geometry.count)
            #expect(zip(route.coordinates, source.geometry).allSatisfy {
                abs($0.longitude - $1[0]) < 1e-9 && abs($0.latitude - $1[1]) < 1e-9
            })
            #expect(route.elevation.map { $0 == nil } == source.elevation.map { $0 == nil })
            #expect(zip(route.elevation, source.elevation).allSatisfy { abs(($0 ?? 0) - ($1 ?? 0)) <= 0.05 + 1e-9 })
            #expect(zip(route.elapsed, source.elapsed).allSatisfy { abs($0 - $1) <= 0.5 })
            #expect(route.legs.map { [$0.from_index, $0.to_index] } == source.legs.map { [$0.from_index, $0.to_index] })
            for (totals, expected) in zip([route.totals] + route.legs.map(\.totals), [source.totals] + source.legs.map(\.totals)) {
                #expect(totals.distance_m == expected.distance_m && totals.ascent_m == expected.ascent_m)
                #expect(abs(totals.seconds - expected.seconds) <= 0.5)
            }
        }
    }
}
