import SwiftUI

/// Photo and note actions below the ride's facts. Adding photos stays available after dismissal.
struct RideOffers: View {
    let photos: RidePhotosModel?
    let dayNote: DayNoteModel?

    private var photoAction: Bool { photos?.canAddPhotos == true }
    private var noteOffer: Bool { dayNote?.offer == true }

    var body: some View {
        VStack(spacing: 0) {
            if let photos, photoAction { RidePhotoOfferRow(model: photos) }
            if photoAction && noteOffer {
                OBCTheme.hairline.frame(height: 1).padding(.leading, 14)
            }
            if let dayNote { DayNoteOfferRow(model: dayNote, photos: photos) }
        }
        .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
        .padding(.top, photoAction || noteOffer ? 20 : 0)
    }
}
