import Foundation
import Observation

@MainActor @Observable
public final class OnboardingProgress {
    public enum Stage: String, Sendable {
        case inactive, sensors, update, route, ride, done
    }

    public private(set) var stage: Stage
    public private(set) var showsReadyNote: Bool
    private let defaults: UserDefaults?
    private static let stageKey = "obc.onboarding.stage"
    private static let readyKey = "obc.onboarding.ready"

    /// A nil store keeps previews and simulator scenarios independent of previous launches.
    public init(defaults: UserDefaults? = nil) {
        self.defaults = defaults
        stage = defaults?.string(forKey: Self.stageKey).flatMap(Stage.init(rawValue:)) ?? .inactive
        showsReadyNote = defaults?.bool(forKey: Self.readyKey) ?? false
    }

    public var isPending: Bool { stage != .inactive && stage != .done }

    public func begin() {
        guard !isPending else { return }
        move(to: .sensors)
        dismissReadyNote()
    }

    public func move(to stage: Stage) {
        self.stage = stage
        defaults?.set(stage.rawValue, forKey: Self.stageKey)
    }

    public func finish() {
        move(to: .done)
        showsReadyNote = true
        defaults?.set(true, forKey: Self.readyKey)
    }

    public func dismissReadyNote() {
        showsReadyNote = false
        defaults?.set(false, forKey: Self.readyKey)
    }

    public func replay() {
        move(to: .sensors)
        dismissReadyNote()
    }

    public func reset() {
        move(to: .inactive)
        dismissReadyNote()
    }
}
