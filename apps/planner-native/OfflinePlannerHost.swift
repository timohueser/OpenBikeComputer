import Foundation
import SwiftUI
import WebKit

@MainActor
final class OfflinePlannerHost: NSObject, ObservableObject, WKNavigationDelegate {
    let webView: WKWebView
    @Published private(set) var failure: String?
    private let server: PlannerHTTPServer
    private var origin: URL?
    private var started = false
    private let port: UInt16
    private var startTask: Task<Void, Never>?

    init(assets: URL, maps: URL, region: String, bounds: [Double], port: UInt16, api: PlannerAPI) throws {
        guard port != 0, region.range(of: "^[a-z][a-z0-9-]{0,63}$", options: .regularExpression) != nil,
              bounds.count == 4, bounds.allSatisfy(\.isFinite),
              -180 <= bounds[0], bounds[0] < bounds[2], bounds[2] <= 180,
              -85 <= bounds[1], bounds[1] < bounds[3], bounds[3] <= 85 else {
            throw plannerSearchError("Invalid offline planner origin or coverage")
        }
        let settings = try JSONSerialization.data(withJSONObject: [
            "region": region, "bounds": bounds, "terrainWorkerUrl": "/dem-worker.js",
        ])
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .default()
        configuration.userContentController.addUserScript(WKUserScript(
            source: "Object.defineProperty(globalThis,'__OBC_PLANNER_CONFIG__',{value:\(String(decoding: settings, as: UTF8.self))});",
            injectionTime: .atDocumentStart, forMainFrameOnly: true))
        webView = WKWebView(frame: .zero, configuration: configuration)
        self.port = port
        server = PlannerHTTPServer(port: port, mounts: ["/": assets, "/maps/": maps], handler: api.respond)
        super.init()
        webView.navigationDelegate = self
        webView.accessibilityLabel = "Offline route planner"
    }

    func start() {
        guard !started else { return }
        started = true
        failure = nil
        startTask = Task { [weak self] in
          guard let self else { return }
          do {
            // Content rules cover subresources and workers as well as navigation.
            let rules = try JSONSerialization.data(withJSONObject: [
                ["trigger": ["url-filter": "^https?://"], "action": ["type": "block"]],
                ["trigger": ["url-filter": "^http://127\\.0\\.0\\.1:\(port)/"], "action": ["type": "ignore-previous-rules"]],
            ])
            guard let list = try await WKContentRuleListStore.default().compileContentRuleList(
                forIdentifier: "offline-planner-\(port)", encodedContentRuleList: String(decoding: rules, as: UTF8.self)) else {
                throw plannerSearchError("Offline network policy did not compile")
            }
            try Task.checkCancellation()
            webView.configuration.userContentController.removeAllContentRuleLists()
            webView.configuration.userContentController.add(list)
            try server.start { [weak self] result in
                Task { @MainActor [weak self] in
                    guard let self, self.started else { return }
                    switch result {
                    case .success(let origin):
                        self.origin = origin
                        self.webView.load(URLRequest(url: origin.appendingPathComponent("planner.html")))
                    case .failure(let error): self.failure = error.localizedDescription
                    }
                }
            }
          } catch { if !Task.isCancelled { failure = error.localizedDescription } }
        }
    }

    func reload() {
        guard let origin else { stop(); start(); return }
        failure = nil
        webView.load(URLRequest(url: origin.appendingPathComponent("planner.html")))
    }

    func stop() {
        started = false
        startTask?.cancel()
        startTask = nil
        webView.stopLoading()
        server.stop()
        origin = nil
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        failure = "The planner stopped. Reopen it to restore the saved draft."
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        if started { failure = error.localizedDescription }
    }

    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
                 decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void) {
        guard let url = action.request.url else { decisionHandler(.cancel); return }
        if url.scheme == "http", url.host == "127.0.0.1", url.port == origin?.port {
            decisionHandler(.allow)
        } else {
            decisionHandler(.cancel)
            if action.navigationType == .linkActivated, ["https", "http"].contains(url.scheme ?? "") {
                UIApplication.shared.open(url)
            }
        }
    }
}

struct OfflinePlannerView: View {
    @ObservedObject var host: OfflinePlannerHost

    var body: some View {
        ZStack {
            PlannerWebView(host: host)
            if let failure = host.failure {
                VStack(spacing: 12) {
                    Text(failure)
                    Button("Reopen planner", action: host.reload)
                }.padding().background(.regularMaterial)
            }
        }
    }
}

private struct PlannerWebView: UIViewRepresentable {
    let host: OfflinePlannerHost
    func makeUIView(context: Context) -> WKWebView { host.start(); return host.webView }
    func updateUIView(_ view: WKWebView, context: Context) {}
}
