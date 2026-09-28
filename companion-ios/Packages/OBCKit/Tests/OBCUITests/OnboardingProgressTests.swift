import Foundation
import Testing
@testable import OBCUI

@Suite @MainActor struct OnboardingProgressTests {
    @Test func interruptedSetupResumesAndCompletionDoesNotReplay() throws {
        let name = "OnboardingProgressTests.\(UUID())"
        let defaults = try #require(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }
        let first = OnboardingProgress(defaults: defaults)
        #expect(!first.isPending)
        first.begin()
        first.move(to: .route)

        let resumed = OnboardingProgress(defaults: defaults)
        #expect(resumed.isPending)
        #expect(resumed.stage == .route)
        resumed.begin()
        #expect(resumed.stage == .route)
        resumed.finish()

        let finished = OnboardingProgress(defaults: defaults)
        #expect(!finished.isPending)
        #expect(finished.showsReadyNote)
        finished.dismissReadyNote()
        #expect(!OnboardingProgress(defaults: defaults).showsReadyNote)
        finished.replay()
        #expect(finished.stage == .sensors)
        #expect(finished.isPending)
        finished.reset()
        #expect(!OnboardingProgress(defaults: defaults).isPending)
    }
}
