import Foundation
import OBCDomain
import OBCWeather

/// Every word the weather screens put on glass, in one place (WX13).
///
/// Two reasons it is a type rather than string literals in views. The vocabulary is *shared* — the
/// settings screen's status line, the diagnostics rows and the retry copy all name the same
/// failures, and a second spelling of "the transfer dropped" is how two screens start disagreeing
/// about what happened. And it is *testable*: the honesty rules this screen exists for (a stale
/// service is never "fine", a drop is never a corruption, the phone never claims the device's
/// authority) are assertions about strings, so the strings need a seam a test can hold.
///
/// Copy tone: plain. No wordplay, no reassurance the state does not support, no explainer line that
/// repeats the row above it.
public enum WeatherCopy {
    // MARK: Names

    /// What the rain-map half of the system is called, everywhere it is named.
    ///
    /// One name, three surfaces. The settings screen used to say "the weather service", the privacy
    /// page "OBC's file storage" and, two bullets later, "OBC's own service" — three names for one
    /// thing, on pages that link to each other, in the exact place a reader is trying to work out
    /// how many parties there are (#1198 review). Written with the article separate so it reads as
    /// a name rather than a slogan.
    public static let serviceName = "OBC weather service"

    /// The main screen's about-group footer. Lives here rather than inline in the view because it
    /// makes the same claim the privacy page makes, and the test that keeps the two in step needs
    /// something to hold.
    public static let aboutFooter =
        "The \(serviceName) is only ever asked for map files — a corridor that names a region, "
        + "never your position. MET Norway receives the position, for the hourly forecast."

    // MARK: Refresh interval

    /// The rider-facing name of a refresh interval — the device's own Off / 15 / 30 / 60 / 120,
    /// short enough to sit in a value column beside "Asks for weather" and read as one sentence.
    public static func refreshLabel(_ refresh: WeatherRefresh) -> String {
        switch refresh {
        case .off: "Off"
        case .every15: "Every 15 min"
        case .every30: "Every 30 min"
        case .every60: "Every hour"
        case .every120: "Every 2 hours"
        }
    }

    /// The value column of the **read-only** refresh row. Three genuinely different not-knowings,
    /// none of them flattened into a plausible-looking interval: an interval this build cannot
    /// name, a device that is not there to ask, and a read that has not landed yet.
    ///
    /// There is deliberately no state for "the rider just changed it here", because they cannot:
    /// the interval is a device setting and the OBC's own Weather screen is its editor. Nothing in
    /// this row is ever this phone's optimism — it is a read or it is an honest blank.
    public static func refreshValue(
        _ refresh: WeatherRefresh?, unknownToThisBuild: Bool, connected: Bool, hasRead: Bool
    ) -> String {
        guard connected else { return "Not connected" }
        guard hasRead else { return "Not read yet" }
        if let refresh { return refreshLabel(refresh) }
        return unknownToThisBuild ? "Set on the device" : "—"
    }

    // MARK: Job outcomes

    public static func outcomeLabel(_ outcome: WeatherJobHistoryEntry.Outcome) -> String {
        switch outcome {
        case .committed: "Delivered"
        case .failed: "Failed"
        case .superseded: "Replaced"
        case .agedOut: "Expired"
        }
    }

    /// The short reason a job ended, in the rider's words. Every case is distinct on purpose —
    /// this is the vocabulary the WX9 engine records, and collapsing two of them here would undo
    /// the split the engine just made.
    public static func failureLabel(_ failure: WeatherJobFailure?) -> String {
        switch failure {
        case .none: "No reason recorded"
        case .noPosition: "The OBC had no GPS fix"
        case .fetchFailed: "Couldn't fetch the forecast"
        case .contextReadFailed: "Couldn't read the request from the OBC"
        case .uploadFailed: "The Bluetooth transfer dropped"
        case .transferCorrupted: "The transfer arrived corrupted"
        case .bundleRejected: "The OBC rejected the weather data"
        case .deviceUnavailable: "The OBC couldn't take it right then"
        case .buildFailed: "Couldn't assemble the weather data"
        case .attemptsExhausted: "Gave up after repeated failures"
        case .superseded: "A newer request replaced it"
        case .agedOut: "Took too long and expired"
        }
    }

    /// The one sentence that answers the question this screen exists for: *whose* fault was it —
    /// the network, the Bluetooth link, or the OBC. Reads off the phase the job reached, which is
    /// exactly where the boundary sits.
    public static func failureExplanation(_ entry: WeatherJobHistoryEntry) -> String? {
        guard entry.outcome == .failed || entry.outcome == .agedOut else { return nil }
        // Two reasons are not about *where* the job got to, so the phase would mis-explain them: a
        // job that ran out of time was not "failing to send", and a fixless request never had a
        // place to fetch for.
        switch entry.failureReason {
        case .agedOut: return "It went out of date before it reached the OBC."
        case .noPosition: return "The OBC couldn't say where it was."
        default: break
        }
        switch entry.phaseReached {
        case .readingContext:
            return "The app never got the request off the OBC."
        case .fetching:
            return "The forecast never reached the phone."
        case .bundleReady, .uploading:
            return "The phone had the weather ready; sending it to the OBC failed."
        }
    }

    /// Why a delivered bundle carried no rain map, in words rather than in Swift's.
    ///
    /// The engine used to flatten this to `String(describing:)`, which put a case name, a label and
    /// a UTC debug date on a diagnostics row, none of which is copy. Each case gets its own
    /// sentence, and the one associated value gets rendered as the time it is.
    ///
    /// **None of these sentences says "no rain".** A genuinely dry corridor is a rain map full of
    /// zeroes, not the absence of one, so it never reaches this function at all.
    public static func noRainMapReasonLabel(_ reason: NoRainMapReason, locale: Locale = .current)
        -> String {
        switch reason {
        case .outOfDomain:
            return "the rain map doesn't reach this area"
        case .uncovered:
            return "no rain-map source covers this area"
        case .expired(let staleAfter):
            return "the rain map expired at \(absolute(staleAfter, locale: locale))"
        case .serviceUnavailable:
            return "the \(serviceName) couldn't be reached"
        case .framesUnavailable:
            return "the rain-map frames couldn't be downloaded"
        }
    }

    public static func phaseLabel(_ phase: WeatherJobPhase) -> String {
        switch phase {
        case .readingContext: "Reading the request"
        case .fetching: "Fetching the forecast"
        case .bundleReady: "Ready to send"
        case .uploading: "Sending to the OBC"
        }
    }

    /// The status line for a job that has not finished — including the honest "waiting" state,
    /// which is a cooldown the rider would otherwise experience as nothing happening.
    public static func pendingLine(_ pending: WeatherJobPending, now: Date) -> String {
        if let notBefore = pending.retryNotBefore, notBefore > now {
            let seconds = Int((notBefore.timeIntervalSince(now)).rounded(.up))
            return "Waiting to retry in \(seconds)s"
        }
        return phaseLabel(pending.phase)
    }

    // MARK: The dataset

    /// "1 km · 9 frames" — the truthful shape of the published dataset, with the lattice's own cell
    /// size and no claim of resolution it does not have.
    ///
    /// There is no tier word here any more, and that is the point: a tier was a *ranking*, and with
    /// one mosaic dataset there is nothing to rank. Naming one on this screen would invite a rider to
    /// wonder which one they got (#1244).
    public static func datasetSummary(_ status: WeatherServiceStatus) -> String {
        [
            cellSize(metres: status.cellSizeMetres),
            "\(status.frameCount) frame\(status.frameCount == 1 ? "" : "s")",
        ].joined(separator: " · ")
    }

    public static func cellSize(metres: UInt16) -> String {
        metres >= 1_000 ? "\(Int((Double(metres) / 1_000).rounded())) km" : "\(metres) m"
    }

    /// The dataset's freshness line: its upstream run, or the deadline it has already passed.
    public static func datasetFreshness(_ status: WeatherServiceStatus, now: Date) -> String {
        guard status.isFresh(at: now) else { return "Stale since \(absolute(status.staleAfter))" }
        return "Source run \(relative(status.referenceTime, now: now))"
    }

    // MARK: Times and sizes

    /// "just now" / "12 min ago" / "3 h ago" / "2 days ago", and the future tense for a clock that
    /// disagrees with the service (which the diagnostics surface rather than silently correct).
    public static func relative(_ date: Date, now: Date) -> String {
        let seconds = now.timeIntervalSince(date)
        if seconds < 0 {
            return "in \(magnitude(-seconds))"
        }
        if seconds < 45 { return "just now" }
        return "\(magnitude(seconds)) ago"
    }

    private static func magnitude(_ seconds: TimeInterval) -> String {
        if seconds < 90 { return "1 min" }
        if seconds < 3_600 { return "\(Int((seconds / 60).rounded())) min" }
        if seconds < 86_400 { return "\(Int((seconds / 3_600).rounded())) h" }
        let days = Int((seconds / 86_400).rounded())
        return "\(days) day\(days == 1 ? "" : "s")"
    }

    /// A wall-clock time in the phone's locale ("14:05" / "2:05 PM") — used where a duration would
    /// be vaguer than the truth ("stale since …").
    public static func absolute(_ date: Date, locale: Locale = .current) -> String {
        let formatter = DateFormatter()
        formatter.locale = locale
        formatter.dateStyle = .none
        formatter.timeStyle = .short
        return formatter.string(from: date)
    }

    /// "14:05 · 3 Aug" — the diagnostics rows, where a bare time would be ambiguous across days.
    public static func stamp(_ date: Date, locale: Locale = .current) -> String {
        let formatter = DateFormatter()
        formatter.locale = locale
        formatter.setLocalizedDateFormatFromTemplate("d MMM HH:mm")
        return formatter.string(from: date)
    }

    /// "18.4 kB" — bundle sizes are always kilobytes under the 256 KiB producer cap.
    public static func kilobytes(_ bytes: Int) -> String {
        String(format: "%.1f kB", Double(max(0, bytes)) / 1_000)
    }

    /// "1.8 s" — the connected-radio times the epic budgets in seconds.
    public static func seconds(milliseconds: Int) -> String {
        String(format: "%.1f s", Double(max(0, milliseconds)) / 1_000)
    }
}
