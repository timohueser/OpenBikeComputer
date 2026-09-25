import SwiftUI
import OBCDomain

/// A day note as page text: the header in caps, the note in body text. The ride detail and
/// the trip review both read a day with it.
public struct DayNoteText: View {
    let header: String
    let note: String

    public init(header: String, note: String) {
        self.header = header
        self.note = note
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            OBCEyebrow(header)
            Text(note)
                .font(.system(.body))
                .lineSpacing(6)
                .foregroundStyle(OBCTheme.ink)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// The ride detail's prompt row, and the writer it opens.
struct DayNoteOfferRow: View {
    let model: DayNoteModel
    let photos: RidePhotosModel?
    @State private var writerShown = false

    var body: some View {
        // The stack stays when the row goes, so the writer it presents is never orphaned.
        VStack(spacing: 0) {
            if model.offer {
                OBCQuietRow(
                    systemImage: "pencil.line",
                    title: model.prompt,
                    onOpen: {
                        model.openWriter()
                        writerShown = true
                    },
                    onDismiss: { withAnimation(.snappy) { model.dismissOffer() } }
                )
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("dayNote.offer")
            }
        }
        .dayNoteWriter(model, photos: photos, isPresented: $writerShown)
    }
}

/// The note on the ride detail: the entry when there is one, "Add a note" after the row was
/// dismissed, nothing while the row still asks.
struct DayNoteEntry: View {
    let model: DayNoteModel
    let photos: RidePhotosModel?
    @State private var writerShown = false

    var body: some View {
        VStack(spacing: 0) {
            if !model.note.isEmpty {
                Button {
                    model.openWriter()
                    writerShown = true
                } label: {
                    DayNoteText(header: model.header, note: model.note)
                        .overlay(alignment: .topTrailing) {
                            Image(systemName: "pencil")
                                .font(.system(.caption, weight: .medium))
                                .foregroundStyle(OBCTheme.secondary)
                        }
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .padding(.top, 22)
                .accessibilityIdentifier("dayNote.entry")
            } else if model.dismissed {
                Button {
                    model.openWriter()
                    writerShown = true
                } label: {
                    Label("Add a note", systemImage: "pencil.line")
                        .font(.system(.subheadline))
                        .foregroundStyle(OBCTheme.secondary)
                }
                .buttonStyle(.plain)
                .padding(.top, 18)
                .accessibilityIdentifier("dayNote.add")
            }
        }
        .dayNoteWriter(model, photos: photos, isPresented: $writerShown)
    }
}

extension View {
    /// The writer is a page of its own: full screen on the phone, a sheet elsewhere.
    @ViewBuilder
    fileprivate func dayNoteWriter(_ model: DayNoteModel, photos: RidePhotosModel?, isPresented: Binding<Bool>) -> some View {
        #if os(iOS)
        fullScreenCover(isPresented: isPresented) { DayNoteWriter(model: model, photos: photos) }
        #else
        sheet(isPresented: isPresented) { DayNoteWriter(model: model, photos: photos) }
        #endif
    }
}

/// The writing page: the day above, the words below. No toolbar but Done; the note saves as the
/// rider types and when the page closes.
public struct DayNoteWriter: View {
    @Bindable var model: DayNoteModel
    let photos: RidePhotosModel?
    @Environment(\.dismiss) private var dismiss
    @FocusState private var focused: Bool

    public init(model: DayNoteModel, photos: RidePhotosModel? = nil) {
        self.model = model
        self.photos = photos
    }

    private static let photoSlots = 4

    public var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 0) {
                OBCEyebrow(model.header)
                Text(model.title)
                    .font(.system(.title, weight: .bold))
                    .foregroundStyle(OBCTheme.ink)
                    .padding(.top, 6)
                if let photos, !photos.photos.isEmpty {
                    photoRow(photos)
                        .padding(.top, 14)
                }
                TextEditor(text: $model.draft)
                    .font(.system(.title3))
                    .lineSpacing(8)
                    .foregroundStyle(OBCTheme.ink)
                    .scrollContentBackground(.hidden)
                    .focused($focused)
                    .padding(.top, 14)
                    .padding(.horizontal, -5)
                    .overlay(alignment: .topLeading) {
                        if model.draft.isEmpty {
                            Text(model.prompt)
                                .font(.system(.title3))
                                .foregroundStyle(OBCTheme.secondary)
                                .padding(.top, 22)
                                .allowsHitTesting(false)
                        }
                    }
                    .accessibilityIdentifier("dayNote.editor")
            }
            .padding(.horizontal, 24)
            .background(OBCTheme.page.ignoresSafeArea())
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                        .fontWeight(.semibold)
                        .accessibilityIdentifier("dayNote.done")
                }
            }
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
        }
        .tint(OBCTheme.tint)
        .onAppear { focused = true }
        .onChange(of: model.draft) { model.draftChanged() }
        .onDisappear { model.save() }
    }

    /// The day's first photos, and how many more there are.
    private func photoRow(_ photos: RidePhotosModel) -> some View {
        let shown = Array(photos.photos.prefix(photos.photos.count > Self.photoSlots ? Self.photoSlots - 1 : Self.photoSlots))
        return HStack(spacing: 6) {
            ForEach(shown) { photo in
                PhotoThumbnail(data: photos.thumbnails[photo.assetID])
                    .frame(width: 84, height: 84)
                    .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusSmall))
            }
            if photos.photos.count > shown.count {
                Text("+\(photos.photos.count - shown.count)")
                    .font(.system(.footnote, weight: .semibold).monospacedDigit())
                    .foregroundStyle(OBCTheme.secondary)
                    .frame(width: 84, height: 84)
                    .background(OBCTheme.surface2)
                    .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusSmall))
            }
        }
        .accessibilityHidden(true)
    }
}
