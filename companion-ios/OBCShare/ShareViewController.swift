import UIKit

/// Puts the shared link and text into `openbikecomputer://place?text=…`, opens the app with it and
/// closes. The app resolves the place: this extension has no UI of its own.
final class ShareViewController: UIViewController {
    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .clear
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        Task { await share() }
    }

    private func share() async {
        let providers = (extensionContext?.inputItems as? [NSExtensionItem] ?? []).flatMap { $0.attachments ?? [] }
        var parts: [String] = []
        if let url = await load(URL.self, from: providers) { parts.append(url.absoluteString) }
        if let text = await load(String.self, from: providers) { parts.append(text) }
        parts = parts.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty }
        guard !parts.isEmpty, let url = Self.appLink(parts.joined(separator: "\n")) else {
            extensionContext?.cancelRequest(withError: CocoaError(.userCancelled))
            return
        }
        // Apple offers no supported API for an extension to open its app; this is the responder-chain workaround.
        let application = sequence(first: self as UIResponder, next: { $0.next }).first { $0 is UIApplication } as? UIApplication
        application?.open(url, options: [:], completionHandler: nil)
        extensionContext?.completeRequest(returningItems: nil, completionHandler: nil)
    }

    /// Every character outside the unreserved set is percent-encoded, so `&`, `+` and `=` in the
    /// text survive the query.
    private static func appLink(_ text: String) -> URL? {
        let unreserved = CharacterSet(charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")
        return text.addingPercentEncoding(withAllowedCharacters: unreserved).flatMap { URL(string: "openbikecomputer://place?text=\($0)") }
    }

    private func load<T: _ObjectiveCBridgeable & Sendable>(_ type: T.Type, from providers: [NSItemProvider]) async -> T?
    where T._ObjectiveCType: NSItemProviderReading {
        guard let provider = providers.first(where: { $0.canLoadObject(ofClass: type) }) else { return nil }
        return await withCheckedContinuation { continuation in
            _ = provider.loadObject(ofClass: type) { @Sendable object, _ in continuation.resume(returning: object) }
        }
    }
}
