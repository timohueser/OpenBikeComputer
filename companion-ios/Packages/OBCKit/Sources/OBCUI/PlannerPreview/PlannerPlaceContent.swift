import SwiftUI
import OBCPlanner

struct PlannerPlaceContent: View {
    let content: PlaceContent
    let name: String
    @State private var photoURL: URL?
    @State private var loading = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let article = content.article {
                ForEach(Array(article.text_pages.enumerated()), id: \.offset) { _, page in
                    Text(verbatim: page).font(.subheadline).fixedSize(horizontal: false, vertical: true)
                }
                HStack {
                    if let url = URL(string: article.attribution.source_url) { Link("Wikipedia contributors", destination: url) }
                    if let url = URL(string: article.attribution.license_url) { Link("Article licence", destination: url) }
                }.font(.caption)
            }
            if let photo = content.photo {
                if let url = photoURL {
                    AsyncImage(url: url) { phase in
                        switch phase {
                        case .success(let image): image.resizable().scaledToFit().frame(maxHeight: 240).accessibilityLabel(name)
                        case .failure: unavailablePhoto
                        default: ProgressView("Loading photo")
                        }
                    }
                } else if loading { ProgressView("Loading photo") } else { unavailablePhoto }
                Text(verbatim: photo.credit.filter { !$0.isEmpty }.joined(separator: " · "))
                    .font(.caption).foregroundStyle(OBCTheme.secondary)
                HStack {
                    if let url = URL(string: photo.source_url) { Link("Photo source", destination: url) }
                    if let url = URL(string: photo.license_url) { Link("Photo licence", destination: url) }
                }.font(.caption)
            }
        }
        .task(id: content.photo) {
            photoURL = nil; loading = content.photo?.online_url != nil
            defer { if !Task.isCancelled { loading = false } }
            do {
                let url = try await content.photo?.onlineURL()
                try Task.checkCancellation()
                photoURL = url
            } catch { if !Task.isCancelled { photoURL = nil } }
        }
    }
    private var unavailablePhoto: some View {
        Text("Photo unavailable. Photos need an internet connection.").font(.caption).foregroundStyle(OBCTheme.secondary)
    }
}
