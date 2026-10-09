import Testing
@testable import OBCUI

struct AcknowledgementsTests {
    @Test func packageNoticesAreReadable() throws {
        let notices = Acknowledgement.bundled
        for title in ["MapLibre", "Protomaps", "Cesium", "Cesium dependencies", "Terminus font"] {
            let notice = try #require(notices.first { $0.title == title })
            #expect(!(try notice.read()).isEmpty)
        }
    }
}
