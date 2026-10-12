import SwiftUI

@MainActor
final class Receiver: ObservableObject {
    enum Phase {
        case scanning
        case done(Route, seconds: Double)
        case failed(String)
    }

    @Published private(set) var phase = Phase.scanning
    @Published private(set) var decoder = FountainDecoder()
    @Published private(set) var started: Date?

    func receive(_ text: String) {
        guard case .scanning = phase else { return }
        let transfer = decoder.crc
        let outcome = decoder.receive(text)
        if outcome == .invalid { return }
        if decoder.crc != transfer { started = .now }
        switch outcome {
        case .complete(let payload):
            let seconds = max(Date.now.timeIntervalSince(started ?? .now), 0.001)
            phase = Route(payload: payload).map { Phase.done($0, seconds: seconds) } ?? Phase.failed("The payload is not a route.")
            UINotificationFeedbackGenerator().notificationOccurred(.success)
        case .corrupt:
            phase = .failed("The checksum does not match.")
        default:
            break
        }
    }

    func reset() {
        phase = .scanning
        decoder = FountainDecoder()
        started = nil
    }
}

struct ContentView: View {
    @StateObject private var receiver = Receiver()

    var body: some View {
        switch receiver.phase {
        case .scanning:
            ScannerView { receiver.receive($0) }
                .ignoresSafeArea()
                .overlay(alignment: .bottom) { progress }
        case let .done(route, seconds: seconds):
            VStack(alignment: .leading, spacing: 12) {
                RouteShape(points: route.points)
                    .stroke(.orange, style: StrokeStyle(lineWidth: 2, lineJoin: .round))
                    .padding()
                Text("\(route.points.count) points · \(route.kilometres, specifier: "%.0f") km")
                Text("\(receiver.decoder.length) bytes in \(seconds, specifier: "%.1f") s · \(Double(receiver.decoder.length) / seconds / 1000, specifier: "%.1f") kB/s")
                Text("\(receiver.decoder.framesSeen) frames for \(receiver.decoder.blockCount) blocks")
                Button("Scan again") { receiver.reset() }.buttonStyle(.borderedProminent)
            }
            .monospacedDigit()
            .padding()
        case let .failed(message):
            VStack(spacing: 12) {
                Text(message)
                Button("Scan again") { receiver.reset() }.buttonStyle(.borderedProminent)
            }
        }
    }

    private var progress: some View {
        let decoder = receiver.decoder
        return VStack(alignment: .leading, spacing: 6) {
            if decoder.blockCount == 0 {
                Text("Point the camera at the sender page.")
            } else {
                ProgressView(value: Double(decoder.rank), total: Double(decoder.blockCount))
                Text("\(decoder.rank) of \(decoder.blockCount) blocks · \(decoder.framesSeen) frames")
                TimelineView(.periodic(from: .now, by: 0.1)) { context in
                    Text("\(context.date.timeIntervalSince(receiver.started ?? context.date), specifier: "%.1f") s")
                }
            }
        }
        .monospacedDigit()
        .padding()
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.regularMaterial)
    }
}
