import SwiftUI

/// The ride page's two one-time offers, photos and the note, as one quiet card low on the page:
/// they add to the ride, so the ride's facts come first. The card goes when both are dismissed.
struct RideOffers: View {
    let photos: RidePhotosModel?
    let dayNote: DayNoteModel?

    private var photoOffer: Bool { photos?.offer != nil }
    private var noteOffer: Bool { dayNote?.offer == true }

    var body: some View {
        VStack(spacing: 0) {
            if let photos { RidePhotoOfferRow(model: photos) }
            if photoOffer && noteOffer {
                OBCTheme.hairline.frame(height: 1).padding(.leading, 14)
            }
            if let dayNote { DayNoteOfferRow(model: dayNote, photos: photos) }
        }
        .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
        .padding(.top, photoOffer || noteOffer ? 20 : 0)
    }
}
