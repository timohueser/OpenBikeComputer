#if canImport(UIKit)
import SwiftUI
import WebKit

struct ReplayWebView: UIViewRepresentable {
    let model: ReplayPlayerModel

    func makeCoordinator() -> Coordinator { Coordinator(model: model) }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.userContentController.add(context.coordinator, name: "replay")
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.isOpaque = false
        view.scrollView.isScrollEnabled = false
        view.navigationDelegate = context.coordinator
        view.accessibilityLabel = "Replay terrain. Drag or pinch to change the camera."
        context.coordinator.start(view)
        return view
    }

    func updateUIView(_ uiView: WKWebView, context: Context) {}

    static func dismantleUIView(_ uiView: WKWebView, coordinator: Coordinator) {
        coordinator.stop()
        uiView.configuration.userContentController.removeScriptMessageHandler(forName: "replay")
        uiView.navigationDelegate = nil
        uiView.stopLoading()
    }

    @MainActor final class Coordinator: NSObject, WKScriptMessageHandler, WKNavigationDelegate {
        private let model: ReplayPlayerModel
        private weak var view: WKWebView?
        private var server: ReplayResourceServer?
        private var origin: URL?
        private var timeout: Task<Void, Never>?
        private var stopped = false

        init(model: ReplayPlayerModel) { self.model = model }

        func start(_ view: WKWebView) {
            self.view = view
            model.send = { [weak self] command in self?.send(command) }
            guard let root = ReplayResourceServer.bundledRoot else {
                model.fail("Replay files are missing. Reinstall the app and try again.")
                return
            }
            let server = ReplayResourceServer(root: root)
            self.server = server
            do {
                try server.start { [weak self] result in
                    Task { @MainActor [weak self] in
                        guard let self, !self.stopped else { return }
                        switch result {
                        case .success(let url): self.origin = url; self.view?.load(URLRequest(url: url))
                        case .failure: self.model.fail("Replay cannot start. Try again.")
                        }
                    }
                }
            } catch { model.fail("Replay cannot start. Try again.") }
            timeout = Task { [weak self] in
                do { try await Task.sleep(for: .seconds(45)) } catch { return }
                guard let self, !self.stopped, self.model.phase == .loading else { return }
                self.model.fail("The map cannot load. Check your connection and try again.")
            }
        }

        func send(_ command: [String: Any]) {
            guard !stopped, JSONSerialization.isValidJSONObject(command) else { return }
            view?.callAsyncJavaScript("window.obcReplay(command)", arguments: ["command": command],
                                      in: nil, in: .page) { [weak self] result in
                guard let self, !self.stopped else { return }
                if case .failure = result { self.model.fail("Replay stopped. Try again.") }
            }
        }

        func stop() {
            guard !stopped else { return }
            send(["type": "destroy"])
            stopped = true
            timeout?.cancel()
            server?.stop()
            server = nil
        }

        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
            guard !stopped, message.frameInfo.isMainFrame,
                  message.frameInfo.securityOrigin.host == "127.0.0.1",
                  message.frameInfo.securityOrigin.port == origin?.port else { return }
            model.receive(message.body)
            if model.phase != .loading { timeout?.cancel() }
        }

        func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
            model.fail("Replay stopped. Try again to continue from this position.")
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            if !stopped { model.fail("Replay cannot load. Try again.") }
        }

        func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
                     decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
            guard let url = navigationAction.request.url else { decisionHandler(.cancel); return }
            if url.scheme == "http", url.host == "127.0.0.1", url.port == origin?.port {
                decisionHandler(.allow)
            } else {
                decisionHandler(.cancel)
                if navigationAction.navigationType == .linkActivated, url.scheme == "https" {
                    UIApplication.shared.open(url)
                }
            }
        }
    }
}
#endif
