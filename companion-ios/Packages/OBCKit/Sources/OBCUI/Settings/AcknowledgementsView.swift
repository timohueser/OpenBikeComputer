import SwiftUI
#if os(iOS)
import UIKit
#endif

struct Acknowledgement: Identifiable {
    let title: String
    let url: URL?
    var id: String { title }

    static var bundled: [Self] {
        [
            Self(title: "Native libraries", url: Bundle.main.url(forResource: "THIRD-PARTY", withExtension: "md")),
            Self(title: "Planner search", url: Bundle.main.url(forResource: "third-party-licenses", withExtension: "txt", subdirectory: "PlannerSearch")),
            Self(title: "MapLibre", url: Bundle.module.url(forResource: "MapLibre-LICENSE", withExtension: "txt", subdirectory: "Map")),
            Self(title: "Protomaps", url: Bundle.module.url(forResource: "Protomaps-LICENSE", withExtension: "txt", subdirectory: "Map")),
            Self(title: "Cesium", url: Bundle.module.url(forResource: "LICENSE", withExtension: "md", subdirectory: "Replay/cesium")),
            Self(title: "Cesium dependencies", url: Bundle.module.url(forResource: "ThirdParty", withExtension: "json", subdirectory: "Replay/cesium")),
            Self(title: "Terminus font", url: Bundle.module.url(forResource: "LICENSE", withExtension: nil, subdirectory: "Terminus")),
        ]
    }

    func read() throws -> String {
        guard let url else { throw CocoaError(.fileNoSuchFile) }
        return try String(contentsOf: url, encoding: .utf8)
    }
}

struct AcknowledgementsView: View {
    var body: some View {
        ScrollView {
            OBCGroupedSection("Third-party software") {
                ForEach(Acknowledgement.bundled) { notice in
                    NavigationLink {
                        AcknowledgementText(notice: notice)
                    } label: {
                        OBCListRow(label: notice.title, showsChevron: true, showsDivider: notice.id != "Terminus font")
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(20)
        }
        .background(OBCTheme.page.ignoresSafeArea())
        .navigationTitle("Acknowledgements")
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
    }
}

private struct AcknowledgementText: View {
    let notice: Acknowledgement
    @State private var text: String?
    @State private var failed = false

    var body: some View {
        Group {
            if let text {
                #if os(iOS)
                AcknowledgementReader(text: text)
                #else
                ScrollView {
                    Text(verbatim: text)
                        .font(.footnote)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(20)
                        .accessibilityIdentifier("acknowledgements.text")
                }
                #endif
            } else if failed {
                Text("This notice is unavailable in this app build.")
                    .padding(20)
                    .accessibilityIdentifier("acknowledgements.error")
            } else {
                ProgressView().padding(20)
            }
        }
        .foregroundStyle(OBCTheme.ink)
        .background(OBCTheme.page.ignoresSafeArea())
        .navigationTitle(notice.title)
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .task {
            do { text = try notice.read() }
            catch { failed = true }
        }
    }
}

#if os(iOS)
/// TextKit lays out the long notice files as the rider scrolls.
private struct AcknowledgementReader: UIViewRepresentable {
    let text: String

    func makeUIView(context: Context) -> UITextView {
        let view = UITextView()
        view.isEditable = false
        view.isSelectable = true
        view.backgroundColor = .clear
        view.font = .preferredFont(forTextStyle: .footnote)
        view.adjustsFontForContentSizeCategory = true
        view.textContainerInset = UIEdgeInsets(top: 20, left: 20, bottom: 20, right: 20)
        view.accessibilityIdentifier = "acknowledgements.text"
        return view
    }

    func updateUIView(_ view: UITextView, context: Context) {
        if view.text != text { view.text = text }
        view.textColor = UIColor(OBCTheme.ink)
    }
}
#endif
