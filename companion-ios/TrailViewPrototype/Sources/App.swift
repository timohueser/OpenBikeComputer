import SwiftUI
import WebKit

@main
struct TrailViewPrototypeApp: App {
    init() { Loopback.shared.start() }
    var body: some Scene { WindowGroup { RootView() } }
}

struct KitPage: Identifiable {
    let name: String
    let path: String
    var id: String { path }
}

/// The spike's plan: today's app at its default and at lighter settings in the four scenes, and one load of the new roads.
private let spikeRuns = ["today", "today-low"].flatMap { v in ["walk", "valley-drive", "zoom-out", "bridge-tunnel"].map { "\(v):\($0)" } } + ["new:walk"]

/// The lever sessions: the phone preset P on the walk, then P with one lever changed (phone-measure.js `levers()`).
private let leversA = ["p", "p-shadows2", "p-shadows0", "p-trees1", "p-gpool1", "p-pool1", "p-objects1", "p-ballast"].map { "\($0):walk" }
private let leversB = ["p", "p-loads1", "p-scale07", "p-terrain08", "p-finest17", "p-fps60"].map { "\($0):walk" } + ["p:valley-drive"]

private let pages = [
    KitPage(name: "Levers A (8 runs)", path: "/phone/?runs=\(leversA.joined(separator: ","))"),
    KitPage(name: "Levers B (7 runs)", path: "/phone/?runs=\(leversB.joined(separator: ","))"),
    KitPage(name: "Measure: the spike runs (9)", path: "/phone/?runs=\(spikeRuns.joined(separator: ","))"),
    // The lighter settings: today's app at its default settings does not survive its load on the iPhone 13.
    KitPage(name: "Soak: the walk for 10 minutes", path: "/phone/?runs=today-low:walk&seconds=600"),
    KitPage(name: "Memory limit of the page", path: "/phone/memory?kind=js"),
    KitPage(name: "Memory limit of the GPU", path: "/phone/memory?kind=gpu"),
    KitPage(name: "Look around (village walk)", path: "/trailview.html?lat=46.820500&lon=8.403000&heading=160.0&pitch=0.0&eye=1.6"),
]

struct RootView: View {
    @AppStorage("kit") private var kit = "https://192.168.2.33:8770"
    @AppStorage("bundled") private var bundled = true
    @State private var open: KitPage?
    @StateObject private var report = NativeReport.shared

    var body: some View {
        Group {
            if let open {
                KitWebView(url: url(open), report: report)
                    .ignoresSafeArea()
                    .statusBarHidden()
                    .persistentSystemOverlays(.hidden)
                    .overlay(alignment: .bottomLeading) {
                        Button { self.open = nil } label: { Image(systemName: "xmark.circle.fill").font(.largeTitle) }
                            .padding(16).opacity(0.5)
                    }
            } else {
                NavigationStack {
                    Form {
                        Section("Measuring kit") {
                            TextField("https://IP:8770", text: $kit)
                                .keyboardType(.URL).autocorrectionDisabled().textInputAutocapitalization(.never)
                            Toggle("TrailView from the app bundle", isOn: $bundled)
                        }
                        Section("Open") { ForEach(pages) { page in Button(page.name) { open = page } } }
                        Section("This phone") {
                            Text("Thermal state: \(report.thermal)")
                            Button("Memory limit of a native app (the app stops)") { NativeMemoryProbe.run(report) }
                            if !report.probe.isEmpty { Text(report.probe) }
                        }
                    }
                    .navigationTitle("TrailView prototype")
                }
            }
        }
        .onAppear {
            UIApplication.shared.isIdleTimerDisabled = true
            Loopback.shared.kit = URL(string: kit)
            // `-open PATH` opens a kit page at launch, for runs that no one taps.
            if open == nil, let path = UserDefaults.standard.string(forKey: "open") { open = KitPage(name: "launch", path: path) }
        }
        .onChange(of: kit) { Loopback.shared.kit = URL(string: kit) }
    }

    /// The kit's pages get a host label, so the session files say which host ran them.
    private func url(_ page: KitPage) -> URL {
        let kitURL = URL(string: kit)
        Loopback.shared.kit = kitURL
        let label = page.path.hasPrefix("/phone") ? (page.path.contains("?") ? "&" : "?") + "host=\(bundled ? "app" : "applan")" : ""
        let origin = bundled ? Loopback.origin : (kitURL?.absoluteString ?? Loopback.origin)
        return URL(string: origin + page.path + label)!
    }
}

/// What only the app can see: the thermal state, and the end of the web content process. Each goes to the kit as an event
/// of the session `native-…`.
@MainActor final class NativeReport: ObservableObject {
    static let shared = NativeReport()
    @Published var thermal = NativeReport.name(ProcessInfo.processInfo.thermalState)
    @Published var probe = ""
    private let session = "native-" + ISO8601DateFormatter().string(from: Date()).filter { $0.isNumber }
    private var timer: Timer?

    private init() {
        NotificationCenter.default.addObserver(forName: ProcessInfo.thermalStateDidChangeNotification, object: nil, queue: .main) { _ in
            Task { @MainActor in NativeReport.shared.sample("changed") }
        }
        timer = Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { _ in Task { @MainActor in NativeReport.shared.sample("every 30 s") } }
    }

    func sample(_ why: String) {
        thermal = Self.name(ProcessInfo.processInfo.thermalState)
        post(["thermal": thermal, "why": why])
    }

    func post(_ value: [String: Any]) {
        guard let kit = Loopback.shared.kit else { return }
        var request = URLRequest(url: kit.appending(path: "result"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "content-type")
        var value = value
        value["thermal"] = Self.name(ProcessInfo.processInfo.thermalState)
        request.httpBody = try? JSONSerialization.data(withJSONObject: ["session": session, "key": "event", "value": value])
        Loopback.shared.session.dataTask(with: request).resume()
    }

    private static func name(_ state: ProcessInfo.ThermalState) -> String {
        switch state {
        case .nominal: "nominal"
        case .fair: "fair"
        case .serious: "serious"
        case .critical: "critical"
        @unknown default: "unknown"
        }
    }
}

/// The native counterpart of the kit's memory probe: it fills memory with random bytes in steps of 64 MiB until iOS stops
/// the app, and posts each step with the memory that iOS says is left, so the last post is the limit.
enum NativeMemoryProbe {
    @MainActor static func run(_ report: NativeReport) {
        guard let kit = Loopback.shared.kit else { return report.probe = "No kit address" }
        let session = "memory-app-native-" + ISO8601DateFormatter().string(from: Date()).filter { $0.isNumber }
        Thread.detachNewThread {
            let step = 64 << 20
            var keep: [UnsafeMutableRawPointer] = []
            while keep.count < 192 {
                let block = UnsafeMutableRawPointer.allocate(byteCount: step, alignment: 16384)
                arc4random_buf(block, step)
                keep.append(block)
                var request = URLRequest(url: kit.appending(path: "result"))
                request.httpMethod = "POST"
                request.setValue("application/json", forHTTPHeaderField: "content-type")
                let value: [String: Any] = ["mib": keep.count * 64, "availableMiB": os_proc_available_memory() >> 20]
                let shown = "Native probe: \(keep.count * 64) MiB, \(os_proc_available_memory() >> 20) MiB left"
                Task { @MainActor in report.probe = shown }
                request.httpBody = try? JSONSerialization.data(withJSONObject: ["session": session, "key": "event", "value": value])
                let posted = DispatchSemaphore(value: 0)
                Loopback.shared.session.dataTask(with: request) { _, _, _ in posted.signal() }.resume()
                posted.wait()
            }
        }
    }
}

struct KitWebView: UIViewRepresentable {
    let url: URL
    let report: NativeReport

    func makeCoordinator() -> Coordinator { Coordinator(report: report) }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        // On disk, as in Safari: a non-persistent store holds TrailView's device cache (up to 296 MiB) in memory.
        configuration.websiteDataStore = .default()
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.isInspectable = true
        view.isOpaque = false
        view.backgroundColor = .black
        view.scrollView.isScrollEnabled = false
        view.scrollView.contentInsetAdjustmentBehavior = .never
        view.navigationDelegate = context.coordinator
        view.load(URLRequest(url: url))
        report.post(["opened": url.absoluteString])
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {}

    final class Coordinator: NSObject, WKNavigationDelegate {
        let report: NativeReport
        init(report: NativeReport) { self.report = report }

        // The kit's certificate is self-signed.
        func webView(_ webView: WKWebView, didReceive challenge: URLAuthenticationChallenge,
                     completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
            if let trust = challenge.protectionSpace.serverTrust { completionHandler(.useCredential, URLCredential(trust: trust)) }
            else { completionHandler(.performDefaultHandling, nil) }
        }

        /// A measuring session goes on after a reload, as in Safari; the memory probe ends at its limit.
        func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
            let page = webView.url?.absoluteString ?? ""
            Task { @MainActor in report.post(["webContentTerminated": page]) }
            if !page.contains("/phone/memory") { webView.reload() }
        }
    }
}
