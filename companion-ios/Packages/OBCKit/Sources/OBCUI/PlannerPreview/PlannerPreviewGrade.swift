#if DEBUG
import Foundation
import OBCDomain

struct PlannerPreviewGrade {
    struct Sample {
        let distance: Double
        let elevation: Double?
    }
    struct Segment {
        let from: Double
        let to: Double
        let start: Double
        let end: Double
        let grade: Double?
    }
    let samples: [Sample]
    let grades: [Double?]
    var distance: Double { samples.last?.distance ?? 0 }

    init(routePoints: [RoutePoint]) {
        let line = MeasuredLine(routePoints: routePoints)
        self.init(samples: zip(line.vertices, routePoints).map {
            Sample(distance: $0.0.distance, elevation: $0.1.elevationIncomplete ? nil : $0.1.elevationMeters)
        })
    }

    // Matches the desktop's 100 m window, shortened at ends and gaps; fragments below 20 m stay unknown.
    init(samples: [Sample]) {
        self.samples = samples
        var grades = [Double?](repeating: nil, count: max(0, samples.count - 1))
        var start = 0
        while start < samples.count {
            guard samples[start].elevation != nil else { start += 1; continue }
            var end = start
            while end + 1 < samples.count, samples[end + 1].elevation != nil { end += 1 }
            var left = start, right = start
            func elevation(_ index: Int, at distance: Double) -> Double {
                let next = min(index + 1, end), length = samples[next].distance - samples[index].distance
                let fraction = length > 0 ? (distance - samples[index].distance) / length : 0
                return samples[index].elevation! + fraction * (samples[next].elevation! - samples[index].elevation!)
            }
            for index in start..<end {
                guard samples[index + 1].distance > samples[index].distance else { continue }
                let middle = (samples[index].distance + samples[index + 1].distance) / 2
                let from = max(samples[start].distance, middle - 50), to = min(samples[end].distance, middle + 50)
                while left < end - 1, samples[left + 1].distance <= from { left += 1 }
                while right < end - 1, samples[right + 1].distance <= to { right += 1 }
                if to - from >= 20 { grades[index] = (elevation(right, at: to) - elevation(left, at: from)) / (to - from) * 100 }
            }
            start = end + 1
        }
        self.grades = grades
    }

    static func band(_ grade: Double?) -> Int {
        guard let grade, grade.isFinite else { return 11 }
        let magnitude = (abs(grade) * 10).rounded() / 10
        let level = [3.0, 6, 10, 15, 20].filter { magnitude >= $0 }.count
        return 5 + (grade < 0 ? -level : level)
    }

    static func label(_ grade: Double?) -> String {
        guard let grade, grade.isFinite else { return "Grade unknown" }
        let rounded = floor(grade * 10 + 0.5) / 10
        return String(format: "%@%.1f%%", rounded > 0 ? "+" : rounded < 0 ? "−" : "", abs(rounded))
    }

    func reading(at fraction: Double) -> (elevation: Double?, grade: Double?) {
        guard samples.count > 1, distance > 0 else { return (nil, nil) }
        let at = min(1, max(0, fraction)) * distance
        let index = max(0, (samples.firstIndex { $0.distance >= at } ?? samples.count - 1) - 1)
        let a = samples[index], b = samples[index + 1]
        guard let start = a.elevation, let end = b.elevation else { return (nil, grades[index]) }
        let amount = b.distance > a.distance ? (at - a.distance) / (b.distance - a.distance) : 0
        return (start + (end - start) * amount, grades[index])
    }

    func segments(in range: ClosedRange<Double>) -> [Segment] {
        guard distance > 0 else { return [] }
        return grades.indices.compactMap { index in
            let a = samples[index], b = samples[index + 1]
            let from = max(a.distance, range.lowerBound * distance), to = min(b.distance, range.upperBound * distance)
            guard to > from, let start = a.elevation, let end = b.elevation else { return nil }
            func height(_ at: Double) -> Double { start + (end - start) * (at - a.distance) / (b.distance - a.distance) }
            return Segment(from: from / distance, to: to / distance, start: height(from), end: height(to), grade: grades[index])
        }
    }

    static let palette: [(light: UInt32, dark: UInt32)] = [
        (0x7030a0, 0xd79bef), (0x5757b6, 0xb4a1ed), (0x356cc1, 0x8eacf1),
        (0x2187bb, 0x6fc4ee), (0x349eac, 0x68d0d9), (0x38834b, 0x86ca8f),
        (0x78a528, 0xb5d76c), (0xc4ac16, 0xeed558), (0xec5036, 0xff987d),
        (0xc72232, 0xff7080), (0x95162c, 0xed4967), (0x77746a, 0xb8b5ac),
    ]
}
#endif
