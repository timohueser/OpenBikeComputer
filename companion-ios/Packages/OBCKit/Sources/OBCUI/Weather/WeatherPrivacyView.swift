import SwiftUI

/// The data-flow story (WX13), in the order the data actually moves.
///
/// Held as values rather than written into the view so it can be *tested*: these sentences are
/// claims about the system's behaviour, and a claim that quietly stops being true is worse than no
/// page at all. The suite pins the load-bearing ones — that the weather service is never told where
/// the rider is, that MET is the only third party that is, and that no location permission is
/// asked for.
public struct WeatherPrivacyCopy: Equatable, Sendable {
    public struct Step: Equatable, Sendable, Identifiable {
        public var id: Int
        public var title: String
        public var body: String
    }

    public var steps: [Step]
    /// What leaves the phone, itemised.
    public var sent: [String]
    /// What does not, itemised — the list that makes the one above meaningful.
    public var notSent: [String]
    public var closing: String

    public static let standard = WeatherPrivacyCopy(
        steps: [
            Step(
                id: 1,
                title: "Your OBC asks",
                body: "When it needs weather, the OBC advertises a request over Bluetooth and "
                    + "hands the phone its GPS position, heading and speed. The app connects for a "
                    + "second or two, reads that, and disconnects."),
            Step(
                id: 2,
                title: "The phone fetches",
                body: "The hourly forecast comes from MET Norway, which receives that position — "
                    + "it is the only third party that ever does. The rain map comes from the "
                    + "\(WeatherCopy.serviceName): the app downloads pieces of ready-made map "
                    + "files. No account, no request the server can tie to you, and no position "
                    + "in any address it asks for."),
            Step(
                id: 3,
                title: "The phone sends it back",
                body: "The app reconnects briefly and uploads one small weather file to the OBC, "
                    + "which checks and stores it. Bluetooth is off again in between and after."),
        ],
        sent: [
            "To MET Norway: the position your OBC reported, so it can answer with that place's "
                + "hourly forecast.",
            "To the \(WeatherCopy.serviceName): which pieces of a rain-map file to send — a "
                + "corridor around your route, which tells the network you are somewhere in that "
                + "region and nothing finer.",
        ],
        notSent: [
            "No account, no sign-in and no identifier: the weather files are the same files for "
                + "everyone, served like any other static download.",
            "No position to the \(WeatherCopy.serviceName) — it has nothing to receive one with, "
                + "and no per-request processing at all.",
            "No location permission from this phone: the app never asks iOS where you are. The "
                + "position comes from your OBC's own GPS.",
            "No ride history, no route contents and no coordinate in the sync diagnostics.",
        ],
        closing: "The position your OBC sent is kept on the phone only until the weather file "
            + "reaches the device, and is excluded from backups while it is there.")

    public init(steps: [Step], sent: [String], notSent: [String], closing: String) {
        self.steps = steps
        self.sent = sent
        self.notSent = notSent
        self.closing = closing
    }
}

/// The rendered page.
public struct WeatherPrivacyView: View {
    private let copy: WeatherPrivacyCopy

    public init(copy: WeatherPrivacyCopy = .standard) {
        self.copy = copy
    }

    public var body: some View {
        ScrollView {
            VStack(spacing: 26) {
                OBCGroupedSection("How it works") {
                    ForEach(copy.steps) { step in
                        stepRow(step, isLast: step.id == copy.steps.count)
                    }
                }
                bulletSection("What leaves your phone", copy.sent, icon: "arrow.up.right", color: OBCTheme.wood)
                bulletSection("What doesn't", copy.notSent, icon: "xmark", color: OBCTheme.forest)
                OBCGroupedSection(footer: copy.closing) {
                    OBCListRow(
                        icon: "lock",
                        iconColor: OBCTheme.forest,
                        label: "Position isn't kept",
                        showsDivider: false
                    )
                }
            }
            .padding(.horizontal, 20)
            .padding(.top, 18)
            .padding(.bottom, 30)
        }
        .background(OBCTheme.parchment.ignoresSafeArea())
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("weather.privacy.screen")
        .navigationTitle("Weather data")
        #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
        #endif
    }

    private func stepRow(_ step: WeatherPrivacyCopy.Step, isLast: Bool) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Text(verbatim: "\(step.id)")
                .font(.obcMono(size: 13, weight: .bold))
                .foregroundStyle(.white)
                .frame(width: 28, height: 28)
                .background(OBCTheme.forest)
                .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusSmall))
            VStack(alignment: .leading, spacing: 4) {
                Text(step.title)
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(OBCTheme.ink)
                Text(step.body)
                    .font(.system(size: 13.5))
                    .foregroundStyle(OBCTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 14)
        .padding(.horizontal, 16)
        .overlay(alignment: .bottom) {
            if !isLast { OBCTheme.screenLine.frame(height: 1).padding(.leading, 56) }
        }
    }

    private func bulletSection(
        _ header: String, _ lines: [String], icon: String, color: Color
    ) -> some View {
        OBCGroupedSection(header) {
            ForEach(Array(lines.enumerated()), id: \.offset) { index, line in
                HStack(alignment: .top, spacing: 12) {
                    Image(systemName: icon)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.white)
                        .frame(width: 28, height: 28)
                        .background(color)
                        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusSmall))
                    Text(line)
                        .font(.system(size: 13.5))
                        .foregroundStyle(OBCTheme.ink)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .padding(.vertical, 12)
                .padding(.horizontal, 16)
                .overlay(alignment: .bottom) {
                    if index != lines.count - 1 {
                        OBCTheme.screenLine.frame(height: 1).padding(.leading, 56)
                    }
                }
            }
        }
    }
}
