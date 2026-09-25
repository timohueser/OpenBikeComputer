import SwiftUI
import WebKit
import OSLog

@main
struct FlyoverApp: App {
    @StateObject private var browser = FlyoverBrowser()
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            ZStack {
                BrowserView(webView: browser.webView).ignoresSafeArea()
                if let error = browser.error {
                    ContentUnavailableView("Cannot load the flyover", systemImage: "wifi.exclamationmark",
                                           description: Text(error))
                }
            }
            .onChange(of: phase) { _, phase in
                if phase != .active {
                    browser.webView.evaluateJavaScript("window.flyover?.pause()")
                    UIApplication.shared.isIdleTimerDisabled = false
                }
            }
        }
    }
}

private struct BrowserView: UIViewRepresentable {
    let webView: WKWebView
    func makeUIView(context: Context) -> WKWebView { webView }
    func updateUIView(_ uiView: WKWebView, context: Context) {}
}

@MainActor
private final class FlyoverBrowser: NSObject, ObservableObject, WKScriptMessageHandler, WKNavigationDelegate {
    let webView: WKWebView
    @Published var error: String?
    private var server: LocalServer?
    private var frameReports: [[String: Any]] = []
    private let log = Logger(subsystem: "com.openbikecomputer.flyover-prototype", category: "performance")

    override init() {
        webView = WKWebView(frame: .zero, configuration: WKWebViewConfiguration())
        super.init()
        webView.isInspectable = true
        webView.scrollView.isScrollEnabled = false
        webView.navigationDelegate = self
        webView.configuration.userContentController.add(self, name: "flyover")
        guard let root = Bundle.main.url(forResource: "Web", withExtension: nil) else {
            error = "Bundled web files are missing. Run npm ci, then rebuild."
            return
        }
        do {
            let server = LocalServer(root: root)
            self.server = server
            try server.start { [weak self] result in
                Task { @MainActor in
                    guard let self else { return }
                    switch result {
                    case .success(let url):
                        var components = URLComponents(url: url, resolvingAgainstBaseURL: false)!
                        if ProcessInfo.processInfo.arguments.contains("-benchmark") {
                            components.query = "benchmark=1"
                        }
                        self.webView.load(URLRequest(url: components.url!))
                    case .failure(let error): self.error = error.localizedDescription
                    }
                }
            }
        } catch { self.error = error.localizedDescription }
    }

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard var report = message.body as? [String: Any] else { return }
        if let playing = report["playing"] as? Bool {
            UIApplication.shared.isIdleTimerDisabled = playing
        }
        report["thermalState"] = String(describing: ProcessInfo.processInfo.thermalState)
        report["lowPowerMode"] = ProcessInfo.processInfo.isLowPowerModeEnabled
        if report["event"] as? String == "frames", report["playing"] as? Bool == true {
            frameReports.append(report)
            if frameReports.count > 120 { frameReports.removeFirst() }
        }
        report["playbackWindows"] = frameReports
        guard JSONSerialization.isValidJSONObject(report),
              let data = try? JSONSerialization.data(withJSONObject: report, options: [.sortedKeys]),
              let text = String(data: data, encoding: .utf8) else { return }
        log.info("Flyover \(text, privacy: .public)")
        let file = URL.documentsDirectory.appendingPathComponent("performance.json")
        do { try data.write(to: file, options: .atomic) }
        catch { log.error("Cannot save performance report: \(error.localizedDescription, privacy: .public)") }
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        self.error = error.localizedDescription
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        UIApplication.shared.isIdleTimerDisabled = false
        error = "The web renderer stopped. Close and reopen OBC Flyover."
    }
}
